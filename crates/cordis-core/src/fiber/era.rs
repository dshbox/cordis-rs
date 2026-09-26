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
    #[cfg(test)]
    probe_successor_drop(successor.fiber.id()).await;
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
        #[cfg(test)]
        probe_converge_drop(handle.fiber.id()).await;
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

// Test-only scheduling probes at the two destruction seams this module
// added containment to. They pause the framework exactly where a test must
// intervene between a barrier observation and a drop; production builds
// never see them. Probes park the owning *task* on a Notify — never a
// completion-runtime worker thread — so concurrently running probe tests
// cannot starve the shared runtime. Slots are keyed by FiberId so parallel
// tests keep independent probes.
#[cfg(test)]
struct ConvergeDropProbe {
    fiber: super::FiberId,
    // Convergence passes that should pass through without parking (the
    // entry and final passes run before the undelivered cleanup's own).
    skip: std::sync::atomic::AtomicUsize,
    armed: std::sync::atomic::AtomicBool,
    entered: std::sync::Arc<tokio::sync::Notify>,
    release: std::sync::Arc<tokio::sync::Notify>,
}

#[cfg(test)]
static CONVERGE_DROP_PROBES: parking_lot::Mutex<Vec<std::sync::Arc<ConvergeDropProbe>>> =
    parking_lot::Mutex::new(Vec::new());

#[cfg(test)]
async fn probe_converge_drop(fiber: &super::FiberId) {
    let probes = CONVERGE_DROP_PROBES.lock().clone();
    for probe in probes {
        if &probe.fiber != fiber {
            continue;
        }
        if probe.skip.load(Ordering::SeqCst) > 0 {
            probe.skip.fetch_sub(1, Ordering::SeqCst);
            continue;
        }
        if !probe.armed.swap(false, Ordering::SeqCst) {
            continue;
        }
        probe.entered.notify_one();
        probe.release.notified().await;
    }
}

#[cfg(test)]
struct SuccessorDropProbe {
    fiber: super::FiberId,
    armed: std::sync::atomic::AtomicBool,
    entered: std::sync::Arc<tokio::sync::Notify>,
    release: std::sync::Arc<tokio::sync::Notify>,
}

#[cfg(test)]
static SUCCESSOR_DROP_PROBES: parking_lot::Mutex<Vec<std::sync::Arc<SuccessorDropProbe>>> =
    parking_lot::Mutex::new(Vec::new());

#[cfg(test)]
async fn probe_successor_drop(fiber: &super::FiberId) {
    let probes = SUCCESSOR_DROP_PROBES.lock().clone();
    for probe in probes {
        if &probe.fiber != fiber {
            continue;
        }
        if !probe.armed.swap(false, Ordering::SeqCst) {
            continue;
        }
        probe.entered.notify_one();
        probe.release.notified().await;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CONVERGE_DROP_PROBES, ConvergeDropProbe, EraHandoff, EraHandoffGuard,
        SUCCESSOR_DROP_PROBES, SuccessorDropProbe, map_spawn_failure,
    };
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

    // The loop-continuation regression's survivor parks its second
    // generation's unload inside a user-registered cleanup, so the undelivered
    // cleanup's convergence barrier provably waits for it. The gate is
    // registered on the second apply only — the first generation's unload
    // runs empty, keeping the swap's own passes unparked.
    struct GatedCleanupDependent {
        applies: Arc<AtomicUsize>,
        entered: Arc<Notify>,
        release: Arc<Notify>,
    }
    impl Plugin for GatedCleanupDependent {
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
        async fn apply(&self, ctx: Context, _: &()) -> Result<(), Infallible> {
            if self.applies.fetch_add(1, Ordering::SeqCst) == 1 {
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
        // The final affected-dependent barrier completed before the handoff
        // was delivered: the survivor's re-apply against the successor's
        // publication is already visible. Checking only after this test's
        // own ready() would mask a skipped barrier — ready() itself drives
        // any pending committed recheck.
        assert_eq!(survivor.state(), FiberState::Active);
        assert_eq!(applies.load(Ordering::SeqCst), 2);
        assert_eq!(successor.ready().await.unwrap(), FiberState::Active);
        assert_eq!(survivor.ready().await.unwrap(), FiberState::Active);

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

    // The undelivered-successor regression's panic source: the change's
    // input, whose destructor panics once armed, parked inside the
    // successor's apply through the public era_swap cancellation path.
    struct PanicOnDropInput {
        armed: Arc<AtomicBool>,
        gate: Option<(Arc<Notify>, Arc<Notify>)>,
    }
    impl Drop for PanicOnDropInput {
        fn drop(&mut self) {
            if self.armed.load(Ordering::SeqCst) {
                panic!("undelivered successor input last Arc dropped during era cleanup");
            }
        }
    }

    struct GatedInputSource;
    impl Plugin for GatedInputSource {
        type Config = ();
        type Input = PanicOnDropInput;
        type PrepareError = Infallible;
        type ApplyError = Infallible;
        fn prepare(&self, _: ()) -> Result<Self::Input, Infallible> {
            unreachable!("the regression installs its input directly")
        }
        async fn apply(&self, ctx: Context, input: &Self::Input) -> Result<(), Infallible> {
            let _ = ctx.provide(Arc::new(EraValue)).unwrap();
            if let Some((entered, release)) = &input.gate {
                entered.notify_one();
                release.notified().await;
            }
            Ok(())
        }
    }

    // Cancelling the public era_swap caller after the owner committed is the
    // one public route to an undelivered successor: the owner's send fails,
    // the handoff guard detaches the cleanup, and the cleanup's trailing
    // successor drop destroys the change's user Input.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_swap_cleanup_contains_the_successor_input_destruction() {
        let root = Context::new();
        let buffer = Arc::new(BufferExporter::new(16, Level::Debug).unwrap());
        let _exporter = root.add_exporter(buffer.clone()).unwrap();
        let armed = Arc::new(AtomicBool::new(false));
        let source = root
            .spawn(PreparedPlugin::from_input(
                GatedInputSource,
                PanicOnDropInput {
                    armed: armed.clone(),
                    gate: None,
                },
            ))
            .await
            .unwrap();
        assert_eq!(source.state(), FiberState::Active);
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

        // The successor parks its apply on the change input's gate: the
        // owner is inside successor creation and has not sent yet.
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let swapping = source.clone();
        let change_input = PanicOnDropInput {
            armed: armed.clone(),
            gate: Some((entered.clone(), release.clone())),
        };
        let swap = tokio::spawn(async move {
            swapping
                .era_swap(PreparedChange::from_input::<GatedInputSource>(change_input))
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
            .await
            .expect("the successor apply parks on the change input's gate");

        // Observe the successor through the registry while it is resident,
        // then cancel the caller: the receiver dies with the task, so the
        // owner's eventual send fails and the guard detaches the cleanup.
        let successor_arc = root
            .root
            .registry
            .snapshot_fibers()
            .into_iter()
            .find(|fiber| {
                !Arc::ptr_eq(fiber, &survivor.fiber)
                    && !Arc::ptr_eq(fiber, &root.root.root_fiber)
                    && !Arc::ptr_eq(fiber, &source.fiber)
            })
            .expect("the gated successor is registry-resident");
        let weak = Arc::downgrade(&successor_arc);
        let successor_id = successor_arc.id().clone();
        drop(successor_arc);
        let probe_entered = Arc::new(Notify::new());
        let probe_release = Arc::new(Notify::new());
        let probe = Arc::new(SuccessorDropProbe {
            fiber: successor_id,
            armed: AtomicBool::new(true),
            entered: probe_entered.clone(),
            release: probe_release.clone(),
        });
        SUCCESSOR_DROP_PROBES.lock().push(probe.clone());
        struct Reset(std::sync::Arc<SuccessorDropProbe>);
        impl Drop for Reset {
            fn drop(&mut self) {
                SUCCESSOR_DROP_PROBES
                    .lock()
                    .retain(|slot| !std::sync::Arc::ptr_eq(slot, &self.0));
            }
        }
        let _reset = Reset(probe);
        swap.abort();
        let _ = swap.await;
        release.notify_one();

        // The cleanup parks between its convergence barrier and the trailing
        // successor drop. The successor's disposal and every transient owner
        // clone complete before the park, so the parked handle is provably
        // the last strong reference once the count settles to one.
        tokio::time::timeout(std::time::Duration::from_secs(2), probe_entered.notified())
            .await
            .expect("the undelivered cleanup reaches its trailing drop");
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while weak.strong_count() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the cleanup's handle is the successor's last Arc");
        armed.store(true, Ordering::SeqCst);
        probe_release.notify_one();

        // The contained destruction and its report complete inside the
        // resumed cleanup; wait for the routed diagnostic rather than an Arc
        // count, which turns zero mid-destruction before the report.
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
        .expect("the cancelled swap's cleanup contains the successor destruction");
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
        // The cleanup's convergence barrier completed before the trailing
        // drop: both eras' publications are gone and the survivor parked on
        // its Missing target after applying against the successor's.
        assert_eq!(survivor.state(), FiberState::Pending);
        assert_eq!(applies.load(Ordering::SeqCst), 2);
        assert_eq!(source.state(), FiberState::Disposed);

        survivor.dispose().await.unwrap();
    }

    // The undelivered cleanup's own convergence pass can meet a disposed
    // dependent's armed Input destructor mid-loop. The pass must contain the
    // panic, still converge the dependents after it, and still destroy the
    // undelivered successor. The target dependent is spawned before the
    // survivor and the single-edge capture preserves spawn order, so the
    // survivor sits after the panic in the loop; the successor's trailing
    // drop runs only once the loop completed, so its destruction is the
    // loop-continuation proof.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn undelivered_cleanup_converges_past_a_dependent_destructor_panic() {
        let root = Context::new();
        let buffer = Arc::new(BufferExporter::new(16, Level::Debug).unwrap());
        let _exporter = root.add_exporter(buffer.clone()).unwrap();
        let source = root
            .spawn(PreparedPlugin::from_input(
                GatedInputSource,
                PanicOnDropInput {
                    armed: Arc::new(AtomicBool::new(false)),
                    gate: None,
                },
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
        let survivor_parked = Arc::new(Notify::new());
        let survivor_release = Arc::new(Notify::new());
        let survivor = root
            .spawn(PreparedPlugin::from_input(
                GatedCleanupDependent {
                    applies: applies.clone(),
                    entered: survivor_parked.clone(),
                    release: survivor_release.clone(),
                },
                (),
            ))
            .await
            .unwrap();
        assert_eq!(survivor.state(), FiberState::Active);
        assert_eq!(applies.load(Ordering::SeqCst), 1);

        // The probe hits every convergence pass, so it must be armed before
        // the swap starts: the entry and final passes see the dependent
        // alive and skip (two hits), and the cleanup pass parks between the
        // dependent's ready() and its handle drop (third hit).
        let weak = Arc::downgrade(&dependent.fiber);
        let probe_entered = Arc::new(Notify::new());
        let probe_release = Arc::new(Notify::new());
        let probe = Arc::new(ConvergeDropProbe {
            fiber: dependent.id(),
            skip: AtomicUsize::new(2),
            armed: AtomicBool::new(true),
            entered: probe_entered.clone(),
            release: probe_release.clone(),
        });
        CONVERGE_DROP_PROBES.lock().push(probe.clone());
        struct Reset(std::sync::Arc<ConvergeDropProbe>);
        impl Drop for Reset {
            fn drop(&mut self) {
                CONVERGE_DROP_PROBES
                    .lock()
                    .retain(|slot| !std::sync::Arc::ptr_eq(slot, &self.0));
            }
        }
        let _reset = Reset(probe);

        // The successor parks its apply on the change input's gate; cancel
        // the caller once it has, so the owner's send fails and the real
        // undelivered cleanup takes over.
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let swapping = source.clone();
        let change_input = PanicOnDropInput {
            armed: Arc::new(AtomicBool::new(false)),
            gate: Some((entered.clone(), release.clone())),
        };
        let swap = tokio::spawn(async move {
            swapping
                .era_swap(PreparedChange::from_input::<GatedInputSource>(change_input))
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
            .await
            .expect("the successor apply parks on the change input's gate");
        let successor_arc = root
            .root
            .registry
            .snapshot_fibers()
            .into_iter()
            .find(|fiber| {
                !Arc::ptr_eq(fiber, &survivor.fiber)
                    && !Arc::ptr_eq(fiber, &dependent.fiber)
                    && !Arc::ptr_eq(fiber, &source.fiber)
                    && !Arc::ptr_eq(fiber, &root.root.root_fiber)
            })
            .expect("the gated successor is registry-resident");
        let successor_weak = Arc::downgrade(&successor_arc);
        drop(successor_arc);
        swap.abort();
        let _ = swap.await;
        release.notify_one();

        // The cleanup pass parked after the dependent's ready(): its capture
        // is committed and the parked handle has not dropped. Dispose the
        // dependent, prove the parked handle is the last strong reference,
        // then arm the destructor panic and let the loop resume into it.
        tokio::time::timeout(std::time::Duration::from_secs(2), probe_entered.notified())
            .await
            .expect("the undelivered cleanup parks at the dependent's drop");
        dependent.dispose().await.unwrap();
        drop(dependent);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while weak.strong_count() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the cleanup pass's handle is the dependent's last Arc");
        armed.store(true, Ordering::SeqCst);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            probe_release.notify_one()
        })
        .await
        .expect("the parked cleanup pass resumes");

        // Exactly one routed diagnostic: the contained panic in the loop.
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let routed = buffer
                    .snapshot()
                    .iter()
                    .any(|record| record.text().contains("Era dependent destruction panicked"));
                if routed {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the cleanup pass contains the dependent's destructor panic");
        let reports = buffer
            .snapshot()
            .into_iter()
            .filter(|record| record.text().contains("Era dependent destruction panicked"))
            .collect::<Vec<_>>();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].level(), Level::Warn);
        assert!(
            reports[0]
                .text()
                .contains("disposed dependent input last Arc dropped during era convergence")
        );

        assert_eq!(
            weak.strong_count(),
            0,
            "the disposed dependent is destroyed"
        );

        // The survivor's second generation parks its unload inside a
        // user-registered cleanup. Service drift alone would also drive the
        // survivor to Pending, so the states above prove continuation only —
        // this gate pins the barrier itself: while it is held, the cleanup's
        // convergence pass cannot pass the survivor, so the successor's
        // trailing drop, which runs only after the loop, cannot have run.
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            survivor_parked.notified(),
        )
        .await
        .expect("the survivor's unload parks in its registered cleanup");
        assert!(
            successor_weak.strong_count() >= 1,
            "the trailing successor drop waits for the survivor's barrier"
        );
        // Releasing the gate is what lets the barrier pass: the successor's
        // destruction is causally downstream of the cleanup's wait on it.
        survivor_release.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while successor_weak.strong_count() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the barrier passed and the undelivered successor is destroyed");
        assert_eq!(survivor.state(), FiberState::Pending);
        assert_eq!(applies.load(Ordering::SeqCst), 2);
        assert_eq!(source.state(), FiberState::Disposed);

        survivor.dispose().await.unwrap();
    }

    // The final convergence pass re-queries dependents after successor
    // visibility; a dependent live at that capture can be disposed and
    // dropped before the pass drops its handle. The probe parks the pass
    // between the dependent's ready() and its drop; the skip counter lets
    // the earlier entry pass (same dependent, still live) run through.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn final_convergence_contains_a_disposed_dependent_last_arc() {
        let root = Context::new();
        let buffer = Arc::new(BufferExporter::new(16, Level::Debug).unwrap());
        let _exporter = root.add_exporter(buffer.clone()).unwrap();
        let source = root
            .spawn(PreparedPlugin::from_input(
                GatedCleanupSource {
                    entered: Arc::new(Notify::new()),
                    release: Arc::new(Notify::new()),
                },
                false,
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

        let weak = Arc::downgrade(&dependent.fiber);
        let probe_entered = Arc::new(Notify::new());
        let probe_release = Arc::new(Notify::new());
        let probe = Arc::new(ConvergeDropProbe {
            fiber: dependent.id(),
            skip: AtomicUsize::new(1),
            armed: AtomicBool::new(true),
            entered: probe_entered.clone(),
            release: probe_release.clone(),
        });
        CONVERGE_DROP_PROBES.lock().push(probe.clone());
        struct Reset(std::sync::Arc<ConvergeDropProbe>);
        impl Drop for Reset {
            fn drop(&mut self) {
                CONVERGE_DROP_PROBES
                    .lock()
                    .retain(|slot| !std::sync::Arc::ptr_eq(slot, &self.0));
            }
        }
        let _reset = Reset(probe);
        let swapping = source.clone();
        let swap = tokio::spawn(async move {
            swapping
                .era_swap(PreparedChange::from_input::<GatedCleanupSource>(false))
                .await
        });

        // The final pass parked after the dependent's ready(): its capture is
        // committed and the parked handle has not dropped yet. Dispose the
        // dependent and drop the public handle, then prove the parked handle
        // is the last strong reference before arming the panic.
        tokio::time::timeout(std::time::Duration::from_secs(2), probe_entered.notified())
            .await
            .expect("the final convergence parks at the dependent's drop");
        dependent.dispose().await.unwrap();
        drop(dependent);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while weak.strong_count() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the final pass's handle is the dependent's last Arc");
        armed.store(true, Ordering::SeqCst);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            probe_release.notify_one()
        })
        .await
        .expect("the parked convergence resumes");

        let successor = tokio::time::timeout(std::time::Duration::from_secs(2), swap)
            .await
            .expect("era replacement owner must finish")
            .expect("disposed dependent Input Drop must be contained")
            .unwrap();
        assert_eq!(successor.ready().await.unwrap(), FiberState::Active);
        assert_eq!(
            weak.strong_count(),
            0,
            "the disposed dependent is destroyed"
        );

        // Exactly one report proves the armed destruction happened in the
        // final pass — the entry pass dropped its Arc while the test still
        // held the public handle, unarmed.
        let reports = buffer
            .snapshot()
            .into_iter()
            .filter(|record| record.text().contains("Era dependent destruction panicked"))
            .collect::<Vec<_>>();
        assert_eq!(
            reports.len(),
            1,
            "one routed diagnostic, from the final pass"
        );
        assert_eq!(reports[0].level(), Level::Warn);
        assert!(
            reports[0]
                .text()
                .contains("disposed dependent input last Arc dropped during era convergence")
        );

        successor.dispose().await.unwrap();
    }
}
