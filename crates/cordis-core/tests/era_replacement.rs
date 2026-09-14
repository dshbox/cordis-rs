//! Issue 36/37 fresh-era replacement and incomplete-successor conformance evidence.

mod common;

use cordis_core::event::{Routing, observer_sync};
use cordis_core::lifecycle::{
    EraSwapError, EraSwapFailure, FiberHandle, FiberState, LifecycleOperation, PluginFailureKind,
};
use cordis_core::service::{ServiceControlError, ServicePublication};
use cordis_core::{Context, Event, InjectSpec, Plugin, PreparedChange, PreparedPlugin, Service};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::{convert::Infallible, future::Future, sync::Arc};

#[derive(Clone)]
struct Probe {
    seen: Arc<Mutex<Vec<u8>>>,
}

impl Plugin for Probe {
    type Config = u8;
    type Input = u8;
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, config: u8) -> Result<u8, Infallible> {
        Ok(config)
    }
    fn apply(
        &self,
        _ctx: Context,
        input: &u8,
    ) -> impl Future<Output = Result<(), Infallible>> + Send {
        self.seen.lock().push(*input);
        std::future::ready(Ok(()))
    }
}

#[tokio::test]
async fn replacement_breaks_identity_while_update_and_restart_do_not() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let old = ctx
        .spawn(PreparedPlugin::from_input(Probe { seen: seen.clone() }, 1))
        .await
        .unwrap();
    let old_id = old.id();
    old.restart().await.unwrap();
    assert_eq!(old.id(), old_id);
    old.update(PreparedChange::from_input::<Probe>(2))
        .await
        .unwrap();
    assert_eq!(old.id(), old_id);
    let successor = old
        .era_swap(PreparedChange::from_input::<Probe>(3))
        .await
        .unwrap();
    assert_eq!(old.state(), FiberState::Disposed);
    assert_ne!(successor.id(), old_id);
    assert_eq!(successor.state(), FiberState::Active);
    assert_eq!(*seen.lock(), vec![1, 1, 2, 3]);
}

#[tokio::test]
async fn closed_source_refuses_without_respawn() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let old = ctx
        .spawn(PreparedPlugin::from_input(Probe { seen: seen.clone() }, 1))
        .await
        .unwrap();
    old.dispose().await.unwrap();
    let err = old
        .era_swap(PreparedChange::from_input::<Probe>(2))
        .await
        .unwrap_err();
    assert!(matches!(err, EraSwapError::Closed));
    assert_eq!(*seen.lock(), vec![1]);
}

struct Pulse;
impl Event for Pulse {
    const NAME: &'static str = "issue36/pulse";
    type Args = ();
    type Output = ();
}

#[derive(Debug)]
struct EraValue(u8);
impl Service for EraValue {
    const NAME: &'static str = "issue36/era-value";
}

struct Child;
impl Plugin for Child {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, _: ()) -> Result<(), Infallible> {
        Ok(())
    }
    fn apply(&self, _: Context, _: &()) -> impl Future<Output = Result<(), Infallible>> + Send {
        std::future::ready(Ok(()))
    }
}

struct EraProbe {
    contexts: Arc<Mutex<Vec<Context>>>,
    hits: Arc<AtomicUsize>,
    cleanups: Arc<AtomicUsize>,
    publications: Arc<Mutex<Vec<ServicePublication<EraValue>>>>,
    child: Arc<Mutex<Option<FiberHandle>>>,
    spawned_child: Arc<AtomicBool>,
}

impl Plugin for EraProbe {
    type Config = u8;
    type Input = u8;
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, value: u8) -> Result<u8, Infallible> {
        Ok(value)
    }
    fn apply(
        &self,
        ctx: Context,
        value: &u8,
    ) -> impl Future<Output = Result<(), Infallible>> + Send {
        let value = *value;
        let contexts = self.contexts.clone();
        let hits = self.hits.clone();
        let cleanups = self.cleanups.clone();
        let publications = self.publications.clone();
        let child = self.child.clone();
        let spawned_child = self.spawned_child.clone();
        async move {
            contexts.lock().push(ctx.clone());
            ctx.on::<Pulse, _>(observer_sync(move |_, _| {
                hits.fetch_add(1, Ordering::SeqCst);
                Ok::<(), Infallible>(())
            }))
            .unwrap();
            publications
                .lock()
                .push(ctx.provide(Arc::new(EraValue(value))).unwrap());
            ctx.effect_sync(move || {
                cleanups.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();
            if !spawned_child.swap(true, Ordering::SeqCst) {
                let spawned = ctx
                    .with_child_scope()
                    .spawn(PreparedPlugin::from_input(Child, ()))
                    .await
                    .unwrap();
                *child.lock() = Some(spawned);
            }
            Ok(())
        }
    }
}

#[tokio::test]
async fn successor_gets_sibling_scope_and_fresh_generation_resources_without_cascading_children() {
    let root = Context::new();
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let hits = Arc::new(AtomicUsize::new(0));
    let cleanups = Arc::new(AtomicUsize::new(0));
    let publications = Arc::new(Mutex::new(Vec::new()));
    let child = Arc::new(Mutex::new(None));
    let spawned_child = Arc::new(AtomicBool::new(false));
    let old = root
        .spawn(PreparedPlugin::from_input(
            EraProbe {
                contexts: contexts.clone(),
                hits: hits.clone(),
                cleanups: cleanups.clone(),
                publications: publications.clone(),
                child: child.clone(),
                spawned_child,
            },
            1,
        ))
        .await
        .unwrap();
    let old_descendant = contexts.lock()[0].with_child_scope().scope();
    let child_fiber_handle = child.lock().as_ref().unwrap().clone();

    let successor = old
        .era_swap(PreparedChange::from_input::<EraProbe>(2))
        .await
        .unwrap();
    assert_eq!(
        cleanups.load(Ordering::SeqCst),
        1,
        "old generation effects drained exactly once"
    );
    assert_eq!(
        child_fiber_handle.state(),
        FiberState::Active,
        "spawned Fibers are not cascaded"
    );
    assert_eq!(
        root.try_service::<EraValue>().unwrap().0,
        2,
        "new publication is visible only after old withdrawal"
    );
    assert!(matches!(
        publications.lock()[0].set(Arc::new(EraValue(9))),
        Err(ServiceControlError::StalePublication { .. })
    ));

    root.emit::<Pulse>(Routing::Scoped(old_descendant), ())
        .await
        .unwrap();
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "old registration is gone and successor is not migrated into old Scope descendants"
    );
    let successor_scope = contexts.lock()[1].scope();
    root.emit::<Pulse>(Routing::Scoped(successor_scope), ())
        .await
        .unwrap();
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "successor owns a fresh sibling-era scoped registration"
    );
    assert_ne!(successor.id(), old.id());
}

struct WantsEraValue {
    seen: Arc<Mutex<Vec<u8>>>,
}
impl Plugin for WantsEraValue {
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
    fn apply(&self, ctx: Context, _: &()) -> impl Future<Output = Result<(), Infallible>> + Send {
        let seen = self.seen.clone();
        async move {
            seen.lock().push(ctx.try_service::<EraValue>().unwrap().0);
            Ok(())
        }
    }
}

#[tokio::test]
async fn return_waits_for_dependents_to_converge_to_the_successors_current_publication() {
    let root = Context::new();
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let provider = root
        .spawn(PreparedPlugin::from_input(
            EraProbe {
                contexts,
                hits: Arc::new(AtomicUsize::new(0)),
                cleanups: Arc::new(AtomicUsize::new(0)),
                publications: Arc::new(Mutex::new(Vec::new())),
                child: Arc::new(Mutex::new(None)),
                spawned_child: Arc::new(AtomicBool::new(true)),
            },
            1,
        ))
        .await
        .unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let dependent = root
        .spawn(PreparedPlugin::from_input(
            WantsEraValue { seen: seen.clone() },
            (),
        ))
        .await
        .unwrap();
    assert_eq!(*seen.lock(), vec![1]);

    let successor = provider
        .era_swap(PreparedChange::from_input::<EraProbe>(2))
        .await
        .unwrap();
    assert_eq!(successor.state(), FiberState::Active);
    assert_eq!(
        dependent.state(),
        FiberState::Active,
        "handoff waits for final dependent quiescence"
    );
    assert_eq!(
        seen.lock().last().copied(),
        Some(2),
        "dependent converged to the successor occurrence, not intermediate absence"
    );
}

#[tokio::test]
async fn racing_replacements_claim_one_live_source_and_attempt_one_successor() {
    let root = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let old = root
        .spawn(PreparedPlugin::from_input(Probe { seen: seen.clone() }, 1))
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        old.era_swap(PreparedChange::from_input::<Probe>(2)),
        old.era_swap(PreparedChange::from_input::<Probe>(3)),
    );
    let successes = usize::from(a.is_ok()) + usize::from(b.is_ok());
    assert_eq!(successes, 1);
    let loser = if let Err(error) = &a {
        error
    } else {
        b.as_ref().unwrap_err()
    };
    assert!(matches!(loser, EraSwapError::Closed));
    assert_eq!(
        seen.lock().len(),
        2,
        "only the source and one successor apply ran"
    );
}

struct GatedEraProvider {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}
impl Plugin for GatedEraProvider {
    type Config = u8;
    type Input = u8;
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, value: u8) -> Result<u8, Infallible> {
        Ok(value)
    }
    fn apply(
        &self,
        ctx: Context,
        value: &u8,
    ) -> impl Future<Output = Result<(), Infallible>> + Send {
        let value = *value;
        let entered = self.entered.clone();
        let release = self.release.clone();
        async move {
            let _publication = ctx.provide(Arc::new(EraValue(value))).unwrap();
            if value == 2 {
                entered.notify_one();
                release.notified().await;
            }
            Ok(())
        }
    }
}

#[tokio::test]
async fn mid_swap_dependent_is_included_by_the_fresh_final_query() {
    let root = Context::new();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let old = root
        .spawn(PreparedPlugin::from_input(
            GatedEraProvider {
                entered: entered.clone(),
                release: release.clone(),
            },
            1,
        ))
        .await
        .unwrap();

    let swap = tokio::spawn({
        let old = old.clone();
        async move {
            old.era_swap(PreparedChange::from_input::<GatedEraProvider>(2))
                .await
        }
    });
    entered.notified().await;

    let seen = Arc::new(Mutex::new(Vec::new()));
    let dependent = root
        .spawn(PreparedPlugin::from_input(
            WantsEraValue { seen: seen.clone() },
            (),
        ))
        .await
        .unwrap();
    assert_eq!(
        dependent.state(),
        FiberState::Pending,
        "the successor publication is still Loading and therefore invisible"
    );
    release.notify_one();

    let successor = swap.await.unwrap().unwrap();
    assert_eq!(successor.state(), FiberState::Active);
    assert_eq!(
        dependent.state(),
        FiberState::Active,
        "the final fresh affected query includes a dependent created mid-swap"
    );
    assert_eq!(seen.lock().last().copied(), Some(2));
}

struct RecursiveEra {
    fiber_handle: Arc<Mutex<Option<FiberHandle>>>,
    refused: Arc<AtomicBool>,
}
impl Plugin for RecursiveEra {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, _: ()) -> Result<(), Infallible> {
        Ok(())
    }
    fn apply(&self, _: Context, _: &()) -> impl Future<Output = Result<(), Infallible>> + Send {
        let fiber_handle = self.fiber_handle.clone();
        let refused = self.refused.clone();
        async move {
            let current = fiber_handle.lock().clone();
            if let Some(current) = current {
                let error = current
                    .era_swap(PreparedChange::from_input::<RecursiveEra>(()))
                    .await
                    .unwrap_err();
                match error {
                    EraSwapError::Recursion(recursion) => {
                        assert_eq!(recursion.operation(), LifecycleOperation::EraSwap);
                        assert_eq!(recursion.fiber_id(), &current.id());
                        refused.store(true, Ordering::SeqCst);
                    }
                    other => panic!("expected typed era recursion refusal, got {other:?}"),
                }
            }
            Ok(())
        }
    }
}

#[tokio::test]
async fn source_settle_recursion_is_preflight_and_leaves_the_era_intact() {
    let root = Context::new();
    let fiber_handle_cell = Arc::new(Mutex::new(None));
    let refused = Arc::new(AtomicBool::new(false));
    let fiber_handle = root
        .spawn(PreparedPlugin::from_input(
            RecursiveEra {
                fiber_handle: fiber_handle_cell.clone(),
                refused: refused.clone(),
            },
            (),
        ))
        .await
        .unwrap();
    *fiber_handle_cell.lock() = Some(fiber_handle.clone());
    fiber_handle.restart().await.unwrap();
    assert!(refused.load(Ordering::SeqCst));
    assert_eq!(fiber_handle.state(), FiberState::Active);
}

struct OrderedEra {
    old_cleaned: Arc<AtomicBool>,
    successor_saw_clean: Arc<AtomicBool>,
}
impl Plugin for OrderedEra {
    type Config = u8;
    type Input = u8;
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, value: u8) -> Result<u8, Infallible> {
        Ok(value)
    }
    fn apply(
        &self,
        ctx: Context,
        value: &u8,
    ) -> impl Future<Output = Result<(), Infallible>> + Send {
        let value = *value;
        let old_cleaned = self.old_cleaned.clone();
        let successor_saw_clean = self.successor_saw_clean.clone();
        async move {
            if value == 1 {
                ctx.effect_sync(move || {
                    old_cleaned.store(true, Ordering::SeqCst);
                })
                .unwrap();
            } else {
                successor_saw_clean.store(old_cleaned.load(Ordering::SeqCst), Ordering::SeqCst);
            }
            Ok(())
        }
    }
}

#[tokio::test]
async fn old_terminal_cleanup_precedes_successor_apply() {
    let root = Context::new();
    let old_cleaned = Arc::new(AtomicBool::new(false));
    let successor_saw_clean = Arc::new(AtomicBool::new(false));
    let old = root
        .spawn(PreparedPlugin::from_input(
            OrderedEra {
                old_cleaned: old_cleaned.clone(),
                successor_saw_clean: successor_saw_clean.clone(),
            },
            1,
        ))
        .await
        .unwrap();

    let successor = old
        .era_swap(PreparedChange::from_input::<OrderedEra>(2))
        .await
        .unwrap();
    assert_eq!(old.state(), FiberState::Disposed);
    assert_eq!(successor.state(), FiberState::Active);
    assert!(old_cleaned.load(Ordering::SeqCst));
    assert!(
        successor_saw_clean.load(Ordering::SeqCst),
        "successor apply cannot begin until the old terminal cleanup barrier is complete"
    );
}

struct OtherContract;
impl Plugin for OtherContract {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, _: ()) -> Result<(), Infallible> {
        Ok(())
    }
    fn apply(&self, _: Context, _: &()) -> impl Future<Output = Result<(), Infallible>> + Send {
        std::future::ready(Ok(()))
    }
}

#[tokio::test]
async fn wrong_contract_refuses_before_source_claim() {
    let root = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let old = root
        .spawn(PreparedPlugin::from_input(Probe { seen: seen.clone() }, 1))
        .await
        .unwrap();
    let old_id = old.id();
    let error = old
        .era_swap(PreparedChange::from_input::<OtherContract>(()))
        .await
        .unwrap_err();
    assert!(matches!(error, EraSwapError::PluginContractMismatch));
    assert_eq!(old.id(), old_id);
    assert_eq!(old.state(), FiberState::Active);
    assert_eq!(*seen.lock(), vec![1]);
}

struct BlockingCleanupEra {
    cleanup_started: Arc<tokio::sync::Notify>,
    cleanup_release: Arc<tokio::sync::Notify>,
    applies: Arc<AtomicUsize>,
}
impl Plugin for BlockingCleanupEra {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, _: ()) -> Result<(), Infallible> {
        Ok(())
    }
    fn apply(&self, ctx: Context, _: &()) -> impl Future<Output = Result<(), Infallible>> + Send {
        let started = self.cleanup_started.clone();
        let release = self.cleanup_release.clone();
        let applies = self.applies.clone();
        async move {
            applies.fetch_add(1, Ordering::SeqCst);
            ctx.effect(move || async move {
                started.notify_one();
                release.notified().await;
            })
            .unwrap();
            Ok(())
        }
    }
}

#[tokio::test]
async fn replacement_losing_to_committed_dispose_refuses_before_successor_allocation() {
    let root = Context::new();
    let cleanup_started = Arc::new(tokio::sync::Notify::new());
    let cleanup_release = Arc::new(tokio::sync::Notify::new());
    let applies = Arc::new(AtomicUsize::new(0));
    let old = root
        .spawn(PreparedPlugin::from_input(
            BlockingCleanupEra {
                cleanup_started: cleanup_started.clone(),
                cleanup_release: cleanup_release.clone(),
                applies: applies.clone(),
            },
            (),
        ))
        .await
        .unwrap();

    let disposing = tokio::spawn({
        let old = old.clone();
        async move {
            old.dispose().await.unwrap();
        }
    });
    cleanup_started.notified().await;
    let error = old
        .era_swap(PreparedChange::from_input::<BlockingCleanupEra>(()))
        .await
        .unwrap_err();
    assert!(matches!(error, EraSwapError::Closed));
    assert_eq!(
        applies.load(Ordering::SeqCst),
        1,
        "a losing replacement allocates no successor"
    );
    cleanup_release.notify_one();
    disposing.await.unwrap();
    assert_eq!(old.state(), FiberState::Disposed);
}

struct FailingEraProvider {
    successor_entered: Arc<tokio::sync::Notify>,
    fail_release: Arc<tokio::sync::Notify>,
    successor_cleaned: Arc<AtomicBool>,
}

impl Plugin for FailingEraProvider {
    type Config = u8;
    type Input = u8;
    type PrepareError = Infallible;
    type ApplyError = std::io::Error;

    fn inject(&self) -> InjectSpec {
        InjectSpec::none()
    }

    fn prepare(&self, value: u8) -> Result<u8, Infallible> {
        Ok(value)
    }

    fn apply(
        &self,
        ctx: Context,
        value: &u8,
    ) -> impl Future<Output = Result<(), std::io::Error>> + Send {
        let value = *value;
        let entered = self.successor_entered.clone();
        let fail_release = self.fail_release.clone();
        let cleaned = self.successor_cleaned.clone();
        async move {
            if value == 1 {
                let _ = ctx.provide(Arc::new(EraValue(1))).unwrap();
                return Ok(());
            }
            ctx.effect_sync(move || {
                cleaned.store(true, Ordering::SeqCst);
            })
            .unwrap();
            entered.notify_one();
            fail_release.notified().await;
            if value == 3 {
                panic!("successor panicked");
            }
            Err(std::io::Error::other("successor returned failure"))
        }
    }
}

struct ReplacementProvider;
impl Plugin for ReplacementProvider {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, _: ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, ctx: Context, _: &()) -> Result<(), Infallible> {
        let _ = ctx.provide(Arc::new(EraValue(9))).unwrap();
        Ok(())
    }
}

struct BlockingDependent {
    entered_nine: Arc<tokio::sync::Notify>,
    release_nine: Arc<tokio::sync::Notify>,
    seen: Arc<Mutex<Vec<u8>>>,
}
impl Plugin for BlockingDependent {
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
        let value = ctx.try_service::<EraValue>().unwrap().0;
        self.seen.lock().push(value);
        if value == 9 {
            self.entered_nine.notify_one();
            self.release_nine.notified().await;
        }
        Ok(())
    }
}

#[tokio::test]
async fn successor_apply_failure_keeps_primary_cause_cleans_successor_and_waits_final_dependents() {
    let root = Context::new();
    let successor_entered = Arc::new(tokio::sync::Notify::new());
    let fail_release = Arc::new(tokio::sync::Notify::new());
    let successor_cleaned = Arc::new(AtomicBool::new(false));
    let old = root
        .spawn(PreparedPlugin::from_input(
            FailingEraProvider {
                successor_entered: successor_entered.clone(),
                fail_release: fail_release.clone(),
                successor_cleaned: successor_cleaned.clone(),
            },
            1,
        ))
        .await
        .unwrap();

    let entered_nine = Arc::new(tokio::sync::Notify::new());
    let release_nine = Arc::new(tokio::sync::Notify::new());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let dependent = root
        .spawn(PreparedPlugin::from_input(
            BlockingDependent {
                entered_nine: entered_nine.clone(),
                release_nine: release_nine.clone(),
                seen: seen.clone(),
            },
            (),
        ))
        .await
        .unwrap();
    assert_eq!(*seen.lock(), vec![1]);

    let swap = tokio::spawn({
        let old = old.clone();
        async move {
            old.era_swap(PreparedChange::from_input::<FailingEraProvider>(2))
                .await
        }
    });
    successor_entered.notified().await;
    assert_eq!(old.state(), FiberState::Disposed);

    let replacement = root
        .spawn(PreparedPlugin::from_input(ReplacementProvider, ()))
        .await
        .unwrap();
    entered_nine.notified().await;
    fail_release.notify_one();
    tokio::task::yield_now().await;

    assert!(
        successor_cleaned.load(Ordering::SeqCst),
        "failed successor cleanup completes before incomplete return"
    );
    assert!(
        !swap.is_finished(),
        "incomplete replacement must wait for the dependent's final current target"
    );

    release_nine.notify_one();
    let error = swap.await.unwrap().unwrap_err();
    match error {
        EraSwapError::Incomplete(EraSwapFailure::SuccessorApply(failure)) => {
            assert_eq!(failure.kind(), PluginFailureKind::ReturnedError);
            assert!(failure.diagnostic().contains("successor returned failure"));
        }
        other => panic!("expected SuccessorApply, got {other:?}"),
    }
    assert_eq!(dependent.state(), FiberState::Active);
    assert_eq!(seen.lock().last().copied(), Some(9));
    replacement.dispose().await.unwrap();
}

#[tokio::test]
async fn successor_apply_panic_remains_the_primary_incomplete_cause_after_cleanup() {
    let root = Context::new();
    let successor_entered = Arc::new(tokio::sync::Notify::new());
    let fail_release = Arc::new(tokio::sync::Notify::new());
    let successor_cleaned = Arc::new(AtomicBool::new(false));
    let old = root
        .spawn(PreparedPlugin::from_input(
            FailingEraProvider {
                successor_entered: successor_entered.clone(),
                fail_release: fail_release.clone(),
                successor_cleaned: successor_cleaned.clone(),
            },
            1,
        ))
        .await
        .unwrap();

    let swap = tokio::spawn({
        let old = old.clone();
        async move {
            old.era_swap(PreparedChange::from_input::<FailingEraProvider>(3))
                .await
        }
    });
    successor_entered.notified().await;
    fail_release.notify_one();
    let error = swap.await.unwrap().unwrap_err();

    assert_eq!(old.state(), FiberState::Disposed);
    assert!(successor_cleaned.load(Ordering::SeqCst));
    match error {
        EraSwapError::Incomplete(EraSwapFailure::SuccessorApply(failure)) => {
            assert_eq!(failure.kind(), PluginFailureKind::Panic);
            assert!(failure.diagnostic().contains("successor panicked"));
        }
        other => panic!("expected panicking SuccessorApply, got {other:?}"),
    }

    // No Failed successor is retained in the typed allocation: bulk removal is
    // an immediate no-op after the incomplete transaction has returned.
    root.remove_plugins::<FailingEraProvider>().await.unwrap();
}

struct CaptureContext {
    slot: Arc<Mutex<Option<Context>>>,
}
impl Plugin for CaptureContext {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, _: ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, ctx: Context, _: &()) -> Result<(), Infallible> {
        *self.slot.lock() = Some(ctx);
        Ok(())
    }
}

#[tokio::test]
async fn replacement_replays_a_closed_spawn_origins_view_without_false_successor_lost() {
    let root = Context::new();
    let origin_ctx = Arc::new(Mutex::new(None));
    let origin = root
        .spawn(PreparedPlugin::from_input(
            CaptureContext {
                slot: origin_ctx.clone(),
            },
            (),
        ))
        .await
        .unwrap();
    let spawning_view = origin_ctx.lock().clone().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let source = spawning_view
        .spawn(PreparedPlugin::from_input(Probe { seen: seen.clone() }, 1))
        .await
        .unwrap();

    origin.dispose().await.unwrap();
    assert_eq!(origin.state(), FiberState::Disposed);
    assert_eq!(source.state(), FiberState::Active);

    let successor = source
        .era_swap(PreparedChange::from_input::<Probe>(2))
        .await
        .unwrap();
    assert_eq!(source.state(), FiberState::Disposed);
    assert_eq!(successor.state(), FiberState::Active);
    assert_eq!(*seen.lock(), vec![1, 2]);
}

struct PreflightSwappingDependent {
    source: Arc<Mutex<Option<FiberHandle>>>,
    outcome: Arc<Mutex<Option<Result<FiberHandle, EraSwapError>>>>,
}
impl Plugin for PreflightSwappingDependent {
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
        let source = self.source.lock().clone();
        if let Some(source) = source {
            let result = source
                .era_swap(PreparedChange::from_input::<EraProbe>(2))
                .await;
            *self.outcome.lock() = Some(result);
        }
        Ok(())
    }
}

#[tokio::test]
async fn entry_dependent_allocation_is_preflighted_before_source_claim() {
    let root = Context::new();
    let source = root
        .spawn(PreparedPlugin::from_input(
            EraProbe {
                contexts: Arc::new(Mutex::new(Vec::new())),
                hits: Arc::new(AtomicUsize::new(0)),
                cleanups: Arc::new(AtomicUsize::new(0)),
                publications: Arc::new(Mutex::new(Vec::new())),
                child: Arc::new(Mutex::new(None)),
                spawned_child: Arc::new(AtomicBool::new(true)),
            },
            1,
        ))
        .await
        .unwrap();
    let source_id = source.id();
    let source_cell = Arc::new(Mutex::new(Some(source.clone())));
    let outcome = Arc::new(Mutex::new(None));

    let dependent = root
        .spawn(PreparedPlugin::from_input(
            PreflightSwappingDependent {
                source: source_cell,
                outcome: outcome.clone(),
            },
            (),
        ))
        .await
        .unwrap();

    let error = outcome
        .lock()
        .take()
        .expect("dependent apply attempted the swap")
        .unwrap_err();
    match error {
        EraSwapError::Recursion(recursion) => {
            assert_eq!(recursion.operation(), LifecycleOperation::EraSwap);
            assert_eq!(recursion.fiber_id(), &dependent.id());
        }
        other => panic!("expected dependent-allocation preflight refusal, got {other:?}"),
    }
    assert_eq!(source.id(), source_id);
    assert_eq!(source.state(), FiberState::Active);
    assert_eq!(dependent.state(), FiberState::Active);
}

#[derive(Debug)]
struct LateEraValue(u8);
impl Service for LateEraValue {
    const NAME: &'static str = "issue38/late-era-value";
}

struct ForeignLateProvider;
impl Plugin for ForeignLateProvider {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, _: ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, ctx: Context, _: &()) -> Result<(), Infallible> {
        let _ = ctx.provide(Arc::new(LateEraValue(9))).unwrap();
        Ok(())
    }
}

struct LatePublishingEra {
    cleanup_started: Arc<tokio::sync::Notify>,
    cleanup_release: Arc<tokio::sync::Notify>,
}
impl Plugin for LatePublishingEra {
    type Config = u8;
    type Input = u8;
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, value: u8) -> Result<u8, Infallible> {
        Ok(value)
    }
    async fn apply(&self, ctx: Context, value: &u8) -> Result<(), Infallible> {
        if *value == 1 {
            let started = self.cleanup_started.clone();
            let release = self.cleanup_release.clone();
            ctx.effect(move || async move {
                started.notify_one();
                release.notified().await;
            })
            .unwrap();
        } else {
            let _ = ctx.provide(Arc::new(LateEraValue(*value))).unwrap();
        }
        Ok(())
    }
}

struct LateDiscoveredSwapper {
    source: Arc<Mutex<Option<FiberHandle>>>,
    attempted: Arc<AtomicBool>,
    outcome: Arc<Mutex<Option<Result<FiberHandle, EraSwapError>>>>,
    seen: Arc<Mutex<Vec<u8>>>,
}
impl Plugin for LateDiscoveredSwapper {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn inject(&self) -> InjectSpec {
        InjectSpec::none().require(LateEraValue::NAME)
    }
    fn prepare(&self, _: ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, ctx: Context, _: &()) -> Result<(), Infallible> {
        self.seen
            .lock()
            .push(ctx.try_service::<LateEraValue>().unwrap().0);
        if !self.attempted.swap(true, Ordering::SeqCst) {
            let source = self
                .source
                .lock()
                .clone()
                .expect("source installed before dependent spawn");
            let result = source
                .era_swap(PreparedChange::from_input::<LatePublishingEra>(2))
                .await;
            *self.outcome.lock() = Some(result);
        }
        Ok(())
    }
}

#[tokio::test]
async fn dependent_discovered_by_fresh_query_repeats_allocation_backstop() {
    let root = Context::new();
    let foreign = root
        .spawn(PreparedPlugin::from_input(ForeignLateProvider, ()))
        .await
        .unwrap();
    let cleanup_started = Arc::new(tokio::sync::Notify::new());
    let cleanup_release = Arc::new(tokio::sync::Notify::new());
    let source = root
        .spawn(PreparedPlugin::from_input(
            LatePublishingEra {
                cleanup_started: cleanup_started.clone(),
                cleanup_release: cleanup_release.clone(),
            },
            1,
        ))
        .await
        .unwrap();
    let source_cell = Arc::new(Mutex::new(Some(source.clone())));
    let attempted = Arc::new(AtomicBool::new(false));
    let outcome = Arc::new(Mutex::new(None));
    let seen = Arc::new(Mutex::new(Vec::new()));

    let dependent_spawn = tokio::spawn({
        let root = root.clone();
        let source = source_cell.clone();
        let attempted = attempted.clone();
        let outcome = outcome.clone();
        let seen = seen.clone();
        async move {
            root.spawn(PreparedPlugin::from_input(
                LateDiscoveredSwapper {
                    source,
                    attempted,
                    outcome,
                    seen,
                },
                (),
            ))
            .await
        }
    });

    cleanup_started.notified().await;
    foreign.dispose().await.unwrap();
    cleanup_release.notify_one();

    let dependent = common::bounded(2_000, dependent_spawn)
        .await
        .expect("late-dependent allocation backstop prevents a self-wait deadlock")
        .unwrap()
        .unwrap();
    let successor = outcome
        .lock()
        .take()
        .expect("the dependent attempted one era swap")
        .expect("late dependent is a dynamic wait backstop, not an entry refusal");

    assert_eq!(source.state(), FiberState::Disposed);
    assert_eq!(successor.state(), FiberState::Active);
    assert_eq!(dependent.state(), FiberState::Active);
    assert_eq!(*seen.lock(), vec![9, 2]);
    successor.dispose().await.unwrap();
}

struct RestartGateEra {
    applies: Arc<AtomicUsize>,
    restart_entered: Arc<tokio::sync::Notify>,
    restart_release: Arc<tokio::sync::Notify>,
}
impl Plugin for RestartGateEra {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, _: ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, _: Context, _: &()) -> Result<(), Infallible> {
        let round = self.applies.fetch_add(1, Ordering::SeqCst) + 1;
        if round == 2 {
            self.restart_entered.notify_one();
            self.restart_release.notified().await;
        }
        Ok(())
    }
}

#[tokio::test]
async fn cancellation_while_waiting_for_source_claim_is_no_effect() {
    let root = Context::new();
    let applies = Arc::new(AtomicUsize::new(0));
    let restart_entered = Arc::new(tokio::sync::Notify::new());
    let restart_release = Arc::new(tokio::sync::Notify::new());
    let source = root
        .spawn(PreparedPlugin::from_input(
            RestartGateEra {
                applies: applies.clone(),
                restart_entered: restart_entered.clone(),
                restart_release: restart_release.clone(),
            },
            (),
        ))
        .await
        .unwrap();

    let restart = tokio::spawn({
        let source = source.clone();
        async move { source.restart().await }
    });
    restart_entered.notified().await;

    let mut swap = Box::pin(source.era_swap(PreparedChange::from_input::<RestartGateEra>(())));
    assert!(matches!(
        futures::poll!(&mut swap),
        std::task::Poll::Pending
    ));
    drop(swap);

    restart_release.notify_one();
    common::bounded(2_000, restart)
        .await
        .expect("restart completes")
        .unwrap()
        .unwrap();
    assert_eq!(source.state(), FiberState::Active);
    assert_eq!(
        applies.load(Ordering::SeqCst),
        2,
        "canceled preclaim swap allocates no successor"
    );
}

#[tokio::test]
async fn later_dispose_waits_only_old_fiber_not_the_winners_successor() {
    let root = Context::new();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let source = root
        .spawn(PreparedPlugin::from_input(
            GatedEraProvider {
                entered: entered.clone(),
                release: release.clone(),
            },
            1,
        ))
        .await
        .unwrap();

    let swap = tokio::spawn({
        let source = source.clone();
        async move {
            source
                .era_swap(PreparedChange::from_input::<GatedEraProvider>(2))
                .await
        }
    });
    entered.notified().await;
    assert_eq!(source.state(), FiberState::Disposed);

    common::bounded(1_000, source.dispose())
        .await
        .expect("later disposal completes while successor apply remains blocked")
        .unwrap();
    assert!(
        !swap.is_finished(),
        "old disposal must not wait on or own successor progress"
    );

    release.notify_one();
    let successor = common::bounded(2_000, swap)
        .await
        .expect("swap completes")
        .unwrap()
        .unwrap();
    successor.dispose().await.unwrap();
}

struct CancelableEra {
    old_cleanup_started: Arc<tokio::sync::Notify>,
    old_cleanup_release: Arc<tokio::sync::Notify>,
    successor_apply_started: Arc<tokio::sync::Notify>,
    successor_apply_release: Arc<tokio::sync::Notify>,
    successor_cleaned: Arc<tokio::sync::Notify>,
    applies: Arc<AtomicUsize>,
}
impl Plugin for CancelableEra {
    type Config = u8;
    type Input = u8;
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, value: u8) -> Result<u8, Infallible> {
        Ok(value)
    }
    async fn apply(&self, ctx: Context, value: &u8) -> Result<(), Infallible> {
        self.applies.fetch_add(1, Ordering::SeqCst);
        if *value == 1 {
            let started = self.old_cleanup_started.clone();
            let release = self.old_cleanup_release.clone();
            ctx.effect(move || async move {
                started.notify_one();
                release.notified().await;
            })
            .unwrap();
        } else {
            self.successor_apply_started.notify_one();
            self.successor_apply_release.notified().await;
            let cleaned = self.successor_cleaned.clone();
            ctx.effect_sync(move || cleaned.notify_one()).unwrap();
        }
        Ok(())
    }
}

async fn cancelable_source() -> (
    Context,
    FiberHandle,
    Arc<tokio::sync::Notify>,
    Arc<tokio::sync::Notify>,
    Arc<tokio::sync::Notify>,
    Arc<tokio::sync::Notify>,
    Arc<tokio::sync::Notify>,
    Arc<AtomicUsize>,
) {
    let root = Context::new();
    let old_started = Arc::new(tokio::sync::Notify::new());
    let old_release = Arc::new(tokio::sync::Notify::new());
    let successor_started = Arc::new(tokio::sync::Notify::new());
    let successor_release = Arc::new(tokio::sync::Notify::new());
    let successor_cleaned = Arc::new(tokio::sync::Notify::new());
    let applies = Arc::new(AtomicUsize::new(0));
    let source = root
        .spawn(PreparedPlugin::from_input(
            CancelableEra {
                old_cleanup_started: old_started.clone(),
                old_cleanup_release: old_release.clone(),
                successor_apply_started: successor_started.clone(),
                successor_apply_release: successor_release.clone(),
                successor_cleaned: successor_cleaned.clone(),
                applies: applies.clone(),
            },
            1,
        ))
        .await
        .unwrap();
    (
        root,
        source,
        old_started,
        old_release,
        successor_started,
        successor_release,
        successor_cleaned,
        applies,
    )
}

#[tokio::test]
async fn cancellation_during_old_cleanup_cannot_stop_committed_replacement_completion() {
    let (
        _root,
        source,
        old_started,
        old_release,
        successor_started,
        successor_release,
        successor_cleaned,
        applies,
    ) = cancelable_source().await;
    let swap = tokio::spawn({
        let source = source.clone();
        async move {
            source
                .era_swap(PreparedChange::from_input::<CancelableEra>(2))
                .await
        }
    });
    old_started.notified().await;
    swap.abort();
    let _ = swap.await;
    old_release.notify_one();
    successor_started.notified().await;
    successor_release.notify_one();
    common::bounded(2_000, successor_cleaned.notified())
        .await
        .expect("undelivered successor is cleaned after old-cleanup cancellation");
    assert_eq!(source.state(), FiberState::Disposed);
    assert_eq!(
        applies.load(Ordering::SeqCst),
        2,
        "committed owner still attempts exactly one successor"
    );
}

#[tokio::test]
async fn cancellation_during_successor_settle_cleans_the_undelivered_successor() {
    let (
        _root,
        source,
        old_started,
        old_release,
        successor_started,
        successor_release,
        successor_cleaned,
        applies,
    ) = cancelable_source().await;
    let swap = tokio::spawn({
        let source = source.clone();
        async move {
            source
                .era_swap(PreparedChange::from_input::<CancelableEra>(2))
                .await
        }
    });
    old_started.notified().await;
    old_release.notify_one();
    successor_started.notified().await;
    swap.abort();
    let _ = swap.await;
    successor_release.notify_one();
    common::bounded(2_000, successor_cleaned.notified())
        .await
        .expect("settled successor is terminally cleaned when caller vanished");
    assert_eq!(source.state(), FiberState::Disposed);
    assert_eq!(applies.load(Ordering::SeqCst), 2);
}

struct CancelableFailureEra {
    cleanup_started: Arc<tokio::sync::Notify>,
    cleanup_release: Arc<tokio::sync::Notify>,
    cleanup_finished: Arc<tokio::sync::Notify>,
    behavior_dropped: Arc<tokio::sync::Notify>,
}
impl Drop for CancelableFailureEra {
    fn drop(&mut self) {
        self.behavior_dropped.notify_one();
    }
}
impl Plugin for CancelableFailureEra {
    type Config = u8;
    type Input = u8;
    type PrepareError = Infallible;
    type ApplyError = std::io::Error;
    fn prepare(&self, value: u8) -> Result<u8, Infallible> {
        Ok(value)
    }
    async fn apply(&self, ctx: Context, value: &u8) -> Result<(), std::io::Error> {
        if *value == 1 {
            return Ok(());
        }
        let started = self.cleanup_started.clone();
        let release = self.cleanup_release.clone();
        let finished = self.cleanup_finished.clone();
        ctx.effect(move || async move {
            started.notify_one();
            release.notified().await;
            finished.notify_one();
        })
        .unwrap();
        let _ = ctx.provide(Arc::new(LateEraValue(7))).unwrap();
        Err(std::io::Error::other("issue38 successor failure"))
    }
}

#[tokio::test]
async fn cancellation_during_failed_successor_cleanup_cannot_interrupt_terminal_unlink() {
    let root = Context::new();
    let cleanup_started = Arc::new(tokio::sync::Notify::new());
    let cleanup_release = Arc::new(tokio::sync::Notify::new());
    let cleanup_finished = Arc::new(tokio::sync::Notify::new());
    let behavior_dropped = Arc::new(tokio::sync::Notify::new());
    let source = root
        .spawn(PreparedPlugin::from_input(
            CancelableFailureEra {
                cleanup_started: cleanup_started.clone(),
                cleanup_release: cleanup_release.clone(),
                cleanup_finished: cleanup_finished.clone(),
                behavior_dropped: behavior_dropped.clone(),
            },
            1,
        ))
        .await
        .unwrap();

    let swap = tokio::spawn({
        let source = source.clone();
        async move {
            source
                .era_swap(PreparedChange::from_input::<CancelableFailureEra>(2))
                .await
        }
    });
    cleanup_started.notified().await;
    assert!(
        root.try_service::<LateEraValue>().is_err(),
        "failed successor publication was withdrawn before its blocking cleanup"
    );
    swap.abort();
    let _ = swap.await;
    cleanup_release.notify_one();
    common::bounded(2_000, cleanup_finished.notified())
        .await
        .expect("failed successor cleanup completes after caller cancellation");
    common::bounded(2_000, behavior_dropped.notified())
        .await
        .expect("failed attempted successor is fully unlinked and dropped");
    assert_eq!(source.state(), FiberState::Disposed);
}

struct BlockingFinalDependent {
    entered_successor: Arc<tokio::sync::Notify>,
    release_successor: Arc<tokio::sync::Notify>,
    seen: Arc<Mutex<Vec<u8>>>,
}
impl Plugin for BlockingFinalDependent {
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
        let value = ctx.try_service::<EraValue>().unwrap().0;
        self.seen.lock().push(value);
        if value == 2 {
            self.entered_successor.notify_one();
            self.release_successor.notified().await;
        }
        Ok(())
    }
}

#[tokio::test]
async fn cancellation_during_final_dependent_recheck_finishes_no_handoff_convergence() {
    let root = Context::new();
    let cleanups = Arc::new(AtomicUsize::new(0));
    let source = root
        .spawn(PreparedPlugin::from_input(
            EraProbe {
                contexts: Arc::new(Mutex::new(Vec::new())),
                hits: Arc::new(AtomicUsize::new(0)),
                cleanups: cleanups.clone(),
                publications: Arc::new(Mutex::new(Vec::new())),
                child: Arc::new(Mutex::new(None)),
                spawned_child: Arc::new(AtomicBool::new(true)),
            },
            1,
        ))
        .await
        .unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let dependent = root
        .spawn(PreparedPlugin::from_input(
            BlockingFinalDependent {
                entered_successor: entered.clone(),
                release_successor: release.clone(),
                seen: seen.clone(),
            },
            (),
        ))
        .await
        .unwrap();
    assert_eq!(*seen.lock(), vec![1]);

    let swap = tokio::spawn({
        let source = source.clone();
        async move {
            source
                .era_swap(PreparedChange::from_input::<EraProbe>(2))
                .await
        }
    });
    entered.notified().await;
    swap.abort();
    let _ = swap.await;
    release.notify_one();

    dependent
        .wait_state(FiberState::Pending, std::time::Duration::from_secs(2))
        .await
        .expect("no-handoff cleanup reconverges dependent to final missing target");
    assert_eq!(source.state(), FiberState::Disposed);
    assert_eq!(cleanups.load(Ordering::SeqCst), 2);
    assert_eq!(seen.lock().last().copied(), Some(2));
}
