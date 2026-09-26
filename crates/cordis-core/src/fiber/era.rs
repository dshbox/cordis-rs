//! Identity-breaking replacement of one live Fiber with one fresh era.
//!
//! Era replacement is a lifecycle transaction, not a same-Fiber settle pass:
//! preflight is effect-free, one live source is claimed, the old terminal
//! barrier completes, one fresh successor is created from the captured creation
//! recipe plus a `PreparedChange`, affected dependents converge to their current
//! targets, and only then may the fresh `FiberHandle` be handed off.

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::context::{RealmKey, Root};
use crate::plugin::{PreparedChange, PreparedPlugin};

use super::spawn::{SpawnError, spawn_prepared_era_successor};
use super::{EraSwapError, EraSwapFailure, Fiber, FiberHandle, LifecycleOperation};

enum EraOwnerFailure {
    Transaction(EraSwapError),
    InputDropPanic(InputDropPanic),
}

// The reply may be sent and then abandoned before the caller polls it.
// Retain a diagnostic owner until the original payload is actually resumed.
struct InputDropPanic {
    payload: Option<Box<dyn std::any::Any + Send>>,
    logger: crate::logger::Logger,
    fiber_name: String,
}

impl InputDropPanic {
    fn report(&self) {
        if let Some(payload) = &self.payload {
            self.logger.error(format!(
                "cordis: era input Drop panicked after caller cancellation (fiber={:?}): {}",
                self.fiber_name,
                crate::contained::payload_text(payload)
            ));
        }
    }

    fn resume(mut self) -> ! {
        resume_unwind(
            self.payload
                .take()
                .expect("panic payload is owned until delivery"),
        )
    }
}

impl Drop for InputDropPanic {
    fn drop(&mut self) {
        if self.payload.is_some() {
            self.report();
        }
    }
}

struct EraHandoff {
    fiber_handle: FiberHandle,
    guard: EraHandoffGuard,
}

impl EraHandoff {
    fn accept(self) -> FiberHandle {
        self.guard.disarm();
        self.fiber_handle
    }
}

struct EraHandoffGuard {
    root: Arc<Root>,
    old_publication_edges: Vec<(String, RealmKey)>,
    source: Arc<Fiber>,
    successor: Option<Arc<Fiber>>,
    attribution: super::settle_ctx::SettleCtx,
}

impl EraHandoffGuard {
    fn new(
        root: Arc<Root>,
        edges: Vec<(String, RealmKey)>,
        source: Arc<Fiber>,
        successor: Arc<Fiber>,
    ) -> Self {
        Self {
            root,
            old_publication_edges: edges,
            source,
            successor: Some(successor),
            attribution: super::settle_ctx::capture_attribution(),
        }
    }

    fn disarm(mut self) {
        self.successor.take();
    }
}

impl Drop for EraHandoffGuard {
    fn drop(&mut self) {
        let Some(successor) = self.successor.take() else {
            return;
        };
        let root = self.root.clone();
        let edges = self.old_publication_edges.clone();
        let source = self.source.clone();
        let attribution = self.attribution.clone();
        crate::effect::detach(super::settle_ctx::with_attribution(
            attribution,
            async move {
                cleanup_undelivered_successor(&root, &edges, &source, FiberHandle::new(successor))
                    .await;
            },
        ));
    }
}

impl FiberHandle {
    /// Replace this live Fiber with one freshly created successor era.
    ///
    /// Exact-allocation recursion is refused before liveness, compatibility,
    /// source claim, or successor allocation. Exactly one racing replacement may
    /// claim a live source; a loser, or a replacement
    /// racing a committed ordinary disposal, returns [`EraSwapError::Closed`]
    /// before successor allocation. Once the source claim commits, framework
    /// ownership completes the old terminal barrier, successor attempt, final
    /// affected-dependent convergence, and handoff-or-cleanup independently of
    /// caller polling.
    ///
    /// The successor replays only the captured Plugin behavior, spawn origin,
    /// and private grouping equivalence with the supplied prepared input replacement.
    /// It receives a fresh [`FiberId`](super::FiberId), freshly resolved exact
    /// dependency edges, a fresh sibling-era Event Scope beneath the captured
    /// origin, fresh generations, and fresh publication occurrences. No old
    /// registration, effect, publication, Scope, or spawned Fiber is migrated.
    pub async fn era_swap(
        &self,
        change: PreparedChange,
    ) -> std::result::Result<FiberHandle, EraSwapError> {
        super::settle_ctx::refuse_recursion(&self.fiber, LifecycleOperation::EraSwap)
            .map_err(EraSwapError::Recursion)?;
        if !self.fiber.is_alive() || self.fiber.disposing.load(Ordering::SeqCst) {
            return Err(EraSwapError::Closed);
        }
        if self.fiber.spawn_state.contract() != Some(change.contract()) {
            return Err(EraSwapError::PluginContractMismatch);
        }

        let recipe = self
            .fiber
            .spawn_state
            .era_recipe()
            .map_err(|_| EraSwapError::Closed)?;
        // Waiting for the lifecycle slot remains precommit: cancellation here
        // changes nothing. The source may publish new Services while another
        // lifecycle pass holds the slot; capture its edges only after winning
        // that slot, before the irreversible terminal claim.
        self.fiber.slot.claim().await;
        if !self.fiber.is_alive() {
            self.fiber.slot.abandon();
            return Err(EraSwapError::Closed);
        }
        // A gated provide holds this journal through slot publication. Holding
        // it across the edge snapshot and terminal claim either includes that
        // publication or closes its admission. Exact set/remove may run in
        // parallel, but neither can add an edge to this source.
        let journal = self.fiber.disposables.lock();
        let old_publication_edges = recipe.root.services.slots_owned_by(&self.fiber);
        // Retain the fallback Registry snapshot until after the journal is
        // released: its final Arc may own user-defined Drop behavior.
        let (mut entry_dependents, retained_snapshot) = recipe
            .root
            .dependents_for_edges_with_retention(&old_publication_edges);
        for dependent in &entry_dependents {
            if std::ptr::eq(self.fiber.as_ref(), dependent.as_ref()) {
                continue;
            }
            if let Err(error) =
                super::settle_ctx::refuse_recursion(dependent, LifecycleOperation::EraSwap)
            {
                drop(journal);
                self.fiber.slot.abandon();
                return Err(EraSwapError::Recursion(error));
            }
        }
        // Fresh-query dependents are not knowable yet. Preserve the current
        // exact-allocation attribution across the postclaim task so their
        // later `ready` backstop can make the same self-wait decision. The task
        // starts on the completion runtime because successor creation can poll
        // arbitrary Plugin apply work.
        let caller_attribution = super::settle_ctx::capture_attribution();
        let already_disposing = self.fiber.claim_terminal();
        drop(journal);
        drop(retained_snapshot);
        entry_dependents.retain(|dependent| !std::ptr::eq(self.fiber.as_ref(), dependent.as_ref()));
        if already_disposing {
            self.fiber.slot.abandon();
            return Err(EraSwapError::Closed);
        }

        let source = self.fiber.clone();
        let completion_root = recipe.root.clone();
        let completion_edges = old_publication_edges.clone();
        let completion_source = source.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        let _join = crate::effect::spawn_completion(super::settle_ctx::with_attribution(
            caller_attribution,
            async move {
                let result = run_committed_replacement(
                    source,
                    recipe,
                    change,
                    old_publication_edges,
                    entry_dependents,
                )
                .await;

                match result {
                    Err(error) => {
                        // An undelivered panic reply reports itself on Drop;
                        // an ordinary transaction error is inert on cancellation.
                        let _ = send.send(Err(error));
                    }
                    Ok(successor) => {
                        let guard = EraHandoffGuard::new(
                            completion_root.clone(),
                            completion_edges.clone(),
                            completion_source.clone(),
                            successor.fiber.clone(),
                        );
                        let offer = EraHandoff {
                            fiber_handle: successor,
                            guard,
                        };
                        let _ = send.send(Ok(offer));
                    }
                }
            },
        ));

        let completion = receive
            .await
            .expect("era replacement owner retains its completion sender");
        match completion {
            Err(EraOwnerFailure::Transaction(error)) => Err(error),
            Err(EraOwnerFailure::InputDropPanic(panic)) => panic.resume(),
            Ok(handoff) => Ok(handoff.accept()),
        }
    }
}

async fn cleanup_undelivered_successor(
    root: &Arc<Root>,
    old_publication_edges: &[(String, RealmKey)],
    source: &Arc<Fiber>,
    successor: FiberHandle,
) {
    let successor_edges = root.services.slots_owned_by(&successor.fiber);
    successor.fiber.dispose().await;
    let mut final_edges = old_publication_edges.to_vec();
    final_edges.extend(successor_edges);
    converge(
        root,
        capture_dependents(
            root,
            &final_edges,
            &[source.as_ref(), successor.fiber.as_ref()],
        ),
    )
    .await;
    // The undelivered successor never reached a caller, so this handle may be
    // its last strong reference: destroying it runs the change's user Input
    // destructor on the detached cleanup's stack.
    let logger = root.logger.logger_for_fiber(&successor.fiber.name);
    crate::contained::contain("Era successor destruction", Some(&logger), || {
        drop(successor)
    });
}

async fn run_committed_replacement(
    source: Arc<Fiber>,
    recipe: super::spawn_state::EraRecipe,
    change: PreparedChange,
    old_publication_edges: Vec<(String, RealmKey)>,
    entry_dependents: Vec<Arc<Fiber>>,
) -> std::result::Result<FiberHandle, EraOwnerFailure> {
    // The source claim already owns the lifecycle slot and closed every
    // generation gate through `disposing`. The Fiber lifecycle owner completes
    // the same full terminal barrier used by ordinary disposal.
    source.complete_claimed_dispose().await;

    // Observe the entry set after old publication withdrawal and before birth.
    converge(&recipe.root, entry_dependents).await;

    let super::spawn_state::EraRecipe {
        plugin,
        name,
        inject,
        plugin_key,
        spawning_ctx,
        root,
    } = recipe;
    let contract = match plugin_key {
        crate::registry::PluginKey::Typed(contract) => contract,
        crate::registry::PluginKey::Anonymous(_) => {
            unreachable!("era recipes are installed only for typed Plugin Fibers")
        }
    };

    // The old user Input is destroyed by into_successor. A panic there must
    // not drop the detached owner before the final affected-dependent barrier.
    // Keep the original payload for the caller after every new Service drift
    // has converged.
    let plugin = match catch_unwind(AssertUnwindSafe(|| plugin.into_successor(change))) {
        Ok(result) => result.unwrap_or_else(|_| {
            panic!("the claimed source retains one compatible successor candidate")
        }),
        Err(payload) => {
            converge_final(&root, &old_publication_edges, &source, None).await;
            return Err(EraOwnerFailure::InputDropPanic(InputDropPanic {
                payload: Some(payload),
                logger: root.logger.logger_for_fiber(&source.name),
                fiber_name: source.name.clone(),
            }));
        }
    };

    let successor = match spawn_prepared_era_successor(
        &spawning_ctx,
        PreparedPlugin {
            plugin,
            name,
            inject,
            contract,
        },
    )
    .await
    {
        Ok(successor) => successor,
        Err(error) => {
            // Creation failures are complete before successor creation returns:
            // pre-allocation refusals allocated nothing, while apply failure or
            // framework invalidation fully disposed and unlinked the attempted
            // Fiber. The era transaction still owns one final fresh dependent
            // query so an Incomplete result cannot outrun target changes that
            // landed while successor creation was in flight.
            converge_final(&root, &old_publication_edges, &source, None).await;
            return Err(EraOwnerFailure::Transaction(EraSwapError::Incomplete(
                map_spawn_failure(error),
            )));
        }
    };

    // Ordinary creation hands off only live quiescent Active/stable Pending.
    // Re-query affected dependents *after* successor visibility so arrivals
    // during the swap are included and all waits target current semantics.
    converge_final(
        &root,
        &old_publication_edges,
        &source,
        Some(&successor.fiber),
    )
    .await;
    Ok(successor)
}

fn map_spawn_failure(error: SpawnError) -> EraSwapFailure {
    match error {
        SpawnError::InitialApply(failure) => EraSwapFailure::SuccessorApply(failure),
        SpawnError::Interrupted => EraSwapFailure::SuccessorLost,
        SpawnError::InactiveContext => {
            unreachable!("era successor replay does not gate on spawn-origin liveness")
        }
    }
}

async fn converge_final(
    root: &Arc<Root>,
    old_publication_edges: &[(String, RealmKey)],
    source: &Arc<Fiber>,
    successor: Option<&Arc<Fiber>>,
) {
    let mut edges = old_publication_edges.to_vec();
    if let Some(successor) = successor {
        edges.extend(root.services.slots_owned_by(successor));
    }
    let mut skip = vec![source.as_ref()];
    if let Some(successor) = successor {
        skip.push(successor.as_ref());
    }
    converge(root, capture_dependents(root, &edges, &skip)).await;
}

async fn converge(root: &Arc<Root>, fibers: Vec<Arc<Fiber>>) {
    for fiber in fibers {
        let handle = FiberHandle::new(fiber);
        let _ = handle.ready().await;
        // A dependent disposed while this era transaction was in flight is
        // unlinked from the Registry, so the handle may hold its last strong
        // reference: its destruction runs a user Input destructor on the era
        // owner's stack. Contain each destruction separately so one panic
        // cannot abort the remaining barrier or the successor handoff.
        let logger = root.logger.logger_for_fiber(&handle.fiber.name);
        crate::contained::contain("Era dependent destruction", Some(&logger), || drop(handle));
    }
}

fn capture_dependents(
    root: &Arc<Root>,
    publication_edges: &[(String, RealmKey)],
    skip: &[&Fiber],
) -> Vec<Arc<Fiber>> {
    let mut captured = root.dependents_for_edges(publication_edges);
    captured.retain(|fiber| !skip.iter().any(|skip| std::ptr::eq(*skip, fiber.as_ref())));
    captured
}

#[cfg(test)]
mod tests {
    use super::{EraHandoff, EraHandoffGuard, map_spawn_failure};
    use crate::context::RealmKey;
    use crate::fiber::{EraSwapError, EraSwapFailure, FiberState, SpawnError};
    use crate::logger::{BufferExporter, Level};
    use crate::{Context, InjectSpec, Plugin, PreparedChange, PreparedPlugin, Service};
    use std::convert::Infallible;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::sync::Notify;

    struct AllocationProbe;
    impl Plugin for AllocationProbe {
        type Config = u8;
        type Input = u8;
        type PrepareError = Infallible;
        type ApplyError = Infallible;
        fn prepare(&self, value: u8) -> Result<u8, Infallible> {
            Ok(value)
        }
        async fn apply(&self, _: Context, _: &u8) -> Result<(), Infallible> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn racing_swaps_admit_exactly_one_successor_fiber() {
        let root = Context::new();
        let source = root
            .spawn(PreparedPlugin::from_input(AllocationProbe, 1))
            .await
            .unwrap();
        let before = root.root.registry.admission_count();
        let (left, right) = tokio::join!(
            source.era_swap(PreparedChange::from_input::<AllocationProbe>(2)),
            source.era_swap(PreparedChange::from_input::<AllocationProbe>(3)),
        );
        assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
        let loser = if let Err(error) = &left {
            error
        } else {
            right.as_ref().unwrap_err()
        };
        assert!(matches!(loser, EraSwapError::Closed));
        assert_eq!(root.root.registry.admission_count(), before + 1);
        let winner = left.ok().or_else(|| right.ok()).unwrap();
        winner.dispose().await.unwrap();
    }

    #[tokio::test]
    async fn unconsumed_success_offer_cleans_the_offered_successor() {
        let root = Context::new();
        let source = root
            .spawn(PreparedPlugin::from_input(AllocationProbe, 1))
            .await
            .unwrap();
        let successor = root
            .spawn(PreparedPlugin::from_input(AllocationProbe, 2))
            .await
            .unwrap();
        let guard = EraHandoffGuard::new(
            root.root.clone(),
            Vec::new(),
            source.fiber.clone(),
            successor.fiber.clone(),
        );
        let offer = EraHandoff {
            fiber_handle: successor.clone(),
            guard,
        };
        let (send, receive) = tokio::sync::oneshot::channel::<Result<EraHandoff, EraSwapError>>();
        assert!(send.send(Ok(offer)).is_ok());
        drop(receive);
        successor
            .wait_state(FiberState::Disposed, std::time::Duration::from_secs(2))
            .await
            .unwrap();
        source.dispose().await.unwrap();
    }

    #[test]
    fn only_framework_interruption_maps_to_successor_lost_at_the_creation_seam() {
        assert!(matches!(
            map_spawn_failure(SpawnError::Interrupted),
            EraSwapFailure::SuccessorLost
        ));
    }

    struct EraValue;
    impl Service for EraValue {
        const NAME: &'static str = "era-convergence-drop";
    }

    // The regression's panic source: a dependent whose Input destructor
    // panics once the era owner is about to destroy its last Arc.
    struct PanicOnLastInputDrop(Arc<AtomicBool>);
    impl Drop for PanicOnLastInputDrop {
        fn drop(&mut self) {
            if self.0.load(Ordering::SeqCst) {
                panic!("disposed dependent input last Arc dropped during era convergence");
            }
        }
    }

    struct DropPanicDependent;
    impl Plugin for DropPanicDependent {
        type Config = ();
        type Input = PanicOnLastInputDrop;
        type PrepareError = Infallible;
        type ApplyError = Infallible;
        fn inject(&self) -> InjectSpec {
            InjectSpec::none().require(EraValue::NAME)
        }
        fn prepare(&self, _: ()) -> Result<Self::Input, Infallible> {
            unreachable!("the regression installs its input directly")
        }
        async fn apply(&self, _: Context, _: &Self::Input) -> Result<(), Infallible> {
            Ok(())
        }
    }

    struct CountingDependent(Arc<AtomicUsize>);
    impl Plugin for CountingDependent {
        type Config = ();
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = Infallible;
        fn inject(&self) -> InjectSpec {
            InjectSpec::none().require(EraValue::NAME)
        }
        fn prepare(&self, _: ()) -> Result<(), Infallible> {
            Ok(())
        }
        async fn apply(&self, _: Context, _: &()) -> Result<(), Infallible> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    // The era source parks its own terminal drain inside a registered
    // cleanup, holding the entry convergence barrier open.
    struct GatedCleanupSource {
        entered: Arc<Notify>,
        release: Arc<Notify>,
    }
    impl Plugin for GatedCleanupSource {
        type Config = ();
        type Input = bool;
        type PrepareError = Infallible;
        type ApplyError = Infallible;
        fn prepare(&self, _: ()) -> Result<bool, Infallible> {
            unreachable!("the regression installs its input directly")
        }
        async fn apply(&self, ctx: Context, parks: &bool) -> Result<(), Infallible> {
            let _ = ctx.provide(Arc::new(EraValue)).unwrap();
            if *parks {
                let entered = self.entered.clone();
                let release = self.release.clone();
                ctx.effect(move || async move {
                    entered.notify_one();
                    release.notified().await;
                })
                .unwrap();
            }
            Ok(())
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn entry_convergence_contains_a_disposed_dependent_last_arc() {
        let root = Context::new();
        let buffer = Arc::new(BufferExporter::new(16, Level::Debug).unwrap());
        let _exporter = root.add_exporter(buffer.clone()).unwrap();
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let source = root
            .spawn(PreparedPlugin::from_input(
                GatedCleanupSource {
                    entered: entered.clone(),
                    release: release.clone(),
                },
                true,
            ))
            .await
            .unwrap();
        assert_eq!(source.state(), FiberState::Active);
        let armed = Arc::new(AtomicBool::new(false));
        let dependent = root
            .spawn(PreparedPlugin::from_input(
                DropPanicDependent,
                PanicOnLastInputDrop(armed.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(dependent.state(), FiberState::Active);
        let applies = Arc::new(AtomicUsize::new(0));
        let survivor = root
            .spawn(PreparedPlugin::from_input(
                CountingDependent(applies.clone()),
                (),
            ))
            .await
            .unwrap();
        assert_eq!(survivor.state(), FiberState::Active);
        assert_eq!(applies.load(Ordering::SeqCst), 1);

        // The entry snapshot retains strong Arcs of both dependents; the
        // swap parks only the source's terminal drain, past that snapshot.
        let weak = Arc::downgrade(&dependent.fiber);
        let swapping = source.clone();
        let swap = tokio::spawn(async move {
            swapping
                .era_swap(PreparedChange::from_input::<GatedCleanupSource>(false))
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
            .await
            .expect("source teardown parks inside its registered cleanup");

        // Dispose the dependent and drop the public handle while the entry
        // snapshot is still retained, then prove that snapshot is the last
        // strong reference before arming the destructor panic.
        dependent.dispose().await.unwrap();
        drop(dependent);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while weak.strong_count() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("entry convergence snapshot is the dependent's last Arc");
        armed.store(true, Ordering::SeqCst);
        release.notify_one();

        let successor = tokio::time::timeout(std::time::Duration::from_secs(2), swap)
            .await
            .expect("era replacement owner must finish")
            .expect("disposed dependent Input Drop must be contained")
            .unwrap();
        assert_eq!(successor.ready().await.unwrap(), FiberState::Active);
        // Final affected-dependent barrier: the survivor re-applied against
        // the successor's fresh publication.
        assert_eq!(survivor.ready().await.unwrap(), FiberState::Active);
        assert_eq!(applies.load(Ordering::SeqCst), 2);

        let reports = buffer
            .snapshot()
            .into_iter()
            .filter(|record| record.text().contains("Era dependent destruction panicked"))
            .collect::<Vec<_>>();
        assert_eq!(
            reports.len(),
            1,
            "one routed diagnostic per destructor panic"
        );
        assert_eq!(reports[0].level(), Level::Warn);
        assert!(
            reports[0]
                .text()
                .contains("disposed dependent input last Arc dropped during era convergence")
        );

        successor.dispose().await.unwrap();
        survivor.dispose().await.unwrap();
    }

    struct PanicOnDropInput;
    impl Drop for PanicOnDropInput {
        fn drop(&mut self) {
            panic!("undelivered successor input last Arc dropped during era cleanup");
        }
    }

    struct PanicInputPlugin;
    impl Plugin for PanicInputPlugin {
        type Config = ();
        type Input = PanicOnDropInput;
        type PrepareError = Infallible;
        type ApplyError = Infallible;
        fn prepare(&self, _: ()) -> Result<Self::Input, Infallible> {
            unreachable!("the regression installs its input directly")
        }
        async fn apply(&self, _: Context, _: &Self::Input) -> Result<(), Infallible> {
            Ok(())
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn undelivered_successor_cleanup_contains_the_successor_input_destruction() {
        let root = Context::new();
        let buffer = Arc::new(BufferExporter::new(16, Level::Debug).unwrap());
        let _exporter = root.add_exporter(buffer.clone()).unwrap();
        let provider = root
            .spawn(PreparedPlugin::from_input(
                GatedCleanupSource {
                    entered: Arc::new(Notify::new()),
                    release: Arc::new(Notify::new()),
                },
                false,
            ))
            .await
            .unwrap();
        let applies = Arc::new(AtomicUsize::new(0));
        let survivor = root
            .spawn(PreparedPlugin::from_input(
                CountingDependent(applies.clone()),
                (),
            ))
            .await
            .unwrap();
        assert_eq!(survivor.state(), FiberState::Active);
        assert_eq!(applies.load(Ordering::SeqCst), 1);
        let successor = root
            .spawn(PreparedPlugin::from_input(
                PanicInputPlugin,
                PanicOnDropInput,
            ))
            .await
            .unwrap();
        assert_eq!(successor.state(), FiberState::Active);

        // Fully dispose the successor before offering it, so the guard's
        // cleanup meets an already-complete terminal barrier and its own
        // trailing handle drop is provably the successor's last reference.
        successor.dispose().await.unwrap();
        let weak = Arc::downgrade(&successor.fiber);
        let guard = EraHandoffGuard::new(
            root.root.clone(),
            vec![(EraValue::NAME.to_string(), RealmKey::DEFAULT)],
            provider.fiber.clone(),
            successor.fiber.clone(),
        );
        let offer = EraHandoff {
            fiber_handle: successor.clone(),
            guard,
        };
        drop(successor);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while weak.strong_count() != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the undelivered offer is the successor's only owner");

        drop(offer);
        // The destruction and its contained report complete inside the
        // detached cleanup; wait for the routed diagnostic rather than an
        // Arc count, which turns zero mid-destruction before the report.
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let routed = buffer
                    .snapshot()
                    .iter()
                    .any(|record| record.text().contains("Era successor destruction panicked"));
                if routed {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("undelivered successor cleanup contains the successor destruction");
        assert_eq!(weak.strong_count(), 0, "the successor is destroyed");

        let reports = buffer
            .snapshot()
            .into_iter()
            .filter(|record| record.text().contains("Era successor destruction panicked"))
            .collect::<Vec<_>>();
        assert_eq!(
            reports.len(),
            1,
            "one routed diagnostic per destructor panic"
        );
        assert_eq!(reports[0].level(), Level::Warn);
        assert!(
            reports[0]
                .text()
                .contains("undelivered successor input last Arc dropped during era cleanup")
        );
        // The cleanup's convergence barrier still completed: the survivor is
        // untouched by the contained destructor panic.
        assert_eq!(survivor.state(), FiberState::Active);
        assert_eq!(applies.load(Ordering::SeqCst), 1);

        provider.dispose().await.unwrap();
        survivor.dispose().await.unwrap();
    }
}
