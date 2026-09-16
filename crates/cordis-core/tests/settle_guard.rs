//! Issue 39 contract evidence for allocation-typed lifecycle-recursion refusal.
//!
//! A doomed lifecycle wait is identified by the exact target Fiber allocation,
//! never by FiberId, Plugin type/group, Scope, or task identity alone. The public
//! refusal is always `LifecycleRecursion` with the exact operation and FiberId.
//! Every potentially hanging probe is bounded so a missing refusal becomes a
//! deterministic assertion failure instead of a suite timeout.

mod common;

use common::bounded;
use cordis_core::effect::EffectRegistration;
use cordis_core::lifecycle::{
    EraSwapError, LifecycleOperation, LifecycleRecursion, ReadyError, RestartError, UpdateError,
    WaitStateError,
};
use cordis_core::{
    Context, FiberHandle, FiberId, FiberState, Plugin, PreparedChange, PreparedPlugin,
};
use parking_lot::Mutex;
use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Notify;

const OPERATIONS: [LifecycleOperation; 7] = [
    LifecycleOperation::Ready,
    LifecycleOperation::WaitState,
    LifecycleOperation::Restart,
    LifecycleOperation::Update,
    LifecycleOperation::EraSwap,
    LifecycleOperation::Dispose,
    LifecycleOperation::RemovePlugins,
];

const MUTATING_OPERATIONS: [LifecycleOperation; 5] = [
    LifecycleOperation::Restart,
    LifecycleOperation::Update,
    LifecycleOperation::EraSwap,
    LifecycleOperation::Dispose,
    LifecycleOperation::RemovePlugins,
];

#[derive(Debug)]
enum Observation {
    Recursion(LifecycleRecursion),
    Success,
    Other,
    TimedOut,
}

fn assert_recursion(observation: Observation, operation: LifecycleOperation, id: &FiberId) {
    let Observation::Recursion(recursion) = observation else {
        panic!("{operation:?} must report typed recursion, got {observation:?}");
    };
    assert_eq!(recursion.operation(), operation);
    assert_eq!(recursion.fiber_id(), id);
}

fn assert_success(observation: Observation, operation: LifecycleOperation) {
    assert!(
        matches!(observation, Observation::Success),
        "unrelated/idle {operation:?} must remain legal, got {observation:?}"
    );
}

async fn observe<P, F>(
    ctx: &Context,
    fiber_handle: &FiberHandle,
    operation: LifecycleOperation,
    prepared: F,
) -> Observation
where
    P: Plugin,
    F: Fn() -> P::Input,
{
    bounded(750, async {
        match operation {
            LifecycleOperation::Ready => match fiber_handle.ready().await {
                Ok(_) => Observation::Success,
                Err(ReadyError::Recursion(recursion)) => Observation::Recursion(recursion),
                Err(_other) => Observation::Other,
            },
            LifecycleOperation::WaitState => {
                match fiber_handle
                    .wait_state(FiberState::Active, Duration::from_secs(5))
                    .await
                {
                    Ok(()) => Observation::Success,
                    Err(WaitStateError::Recursion(recursion)) => Observation::Recursion(recursion),
                    Err(_other) => Observation::Other,
                }
            }
            LifecycleOperation::Restart => match fiber_handle.restart().await {
                Ok(()) => Observation::Success,
                Err(RestartError::Recursion(recursion)) => Observation::Recursion(recursion),
                Err(_other) => Observation::Other,
            },
            LifecycleOperation::Update => {
                match fiber_handle
                    .update(PreparedChange::from_input::<P>(prepared()))
                    .await
                {
                    Ok(_) => Observation::Success,
                    Err(UpdateError::Recursion(recursion)) => Observation::Recursion(recursion),
                    Err(_other) => Observation::Other,
                }
            }
            LifecycleOperation::EraSwap => {
                match fiber_handle
                    .era_swap(PreparedChange::from_input::<P>(prepared()))
                    .await
                {
                    Ok(_) => Observation::Success,
                    Err(EraSwapError::Recursion(recursion)) => Observation::Recursion(recursion),
                    Err(_other) => Observation::Other,
                }
            }
            LifecycleOperation::Dispose => match fiber_handle.dispose().await {
                Ok(()) => Observation::Success,
                Err(recursion) => Observation::Recursion(recursion),
            },
            LifecycleOperation::RemovePlugins => match ctx.remove_plugins::<P>().await {
                Ok(()) => Observation::Success,
                Err(recursion) => Observation::Recursion(recursion),
            },
        }
    })
    .await
    .unwrap_or(Observation::TimedOut)
}

// ---------------------------------------------------------------------------
// Apply attribution: all seven entries reject the exact allocation.
// ---------------------------------------------------------------------------

struct ApplyProbe {
    fiber_handle: Arc<Mutex<Option<FiberHandle>>>,
    seen: Arc<Mutex<Vec<(LifecycleOperation, Observation)>>>,
}

impl Plugin for ApplyProbe {
    type Config = LifecycleOperation;
    type Input = LifecycleOperation;
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, operation: LifecycleOperation) -> Result<LifecycleOperation, Infallible> {
        Ok(operation)
    }

    fn apply(
        &self,
        ctx: Context,
        operation: &LifecycleOperation,
    ) -> impl Future<Output = Result<(), Infallible>> + Send {
        let fiber_handle = self.fiber_handle.lock().clone();
        let seen = self.seen.clone();
        let operation = *operation;
        async move {
            if let Some(fiber_handle) = fiber_handle {
                let observation =
                    observe::<ApplyProbe, _>(&ctx, &fiber_handle, operation, || operation).await;
                seen.lock().push((operation, observation));
            }
            Ok(())
        }
    }
}

struct SpawnedApplyProbe {
    fiber_handle: Arc<Mutex<Option<FiberHandle>>>,
    seen: Arc<Mutex<Vec<(LifecycleOperation, Observation)>>>,
}

impl Plugin for SpawnedApplyProbe {
    type Config = LifecycleOperation;
    type Input = LifecycleOperation;
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, operation: LifecycleOperation) -> Result<LifecycleOperation, Infallible> {
        Ok(operation)
    }

    fn apply(
        &self,
        ctx: Context,
        operation: &LifecycleOperation,
    ) -> impl Future<Output = Result<(), Infallible>> + Send {
        let fiber_handle = self.fiber_handle.lock().clone();
        let seen = self.seen.clone();
        let operation = *operation;
        async move {
            if let Some(fiber_handle) = fiber_handle {
                let task_ctx = ctx.clone();
                let observation = ctx
                    .spawn_attributed(async move {
                        observe::<SpawnedApplyProbe, _>(&task_ctx, &fiber_handle, operation, || {
                            operation
                        })
                        .await
                    })
                    .await
                    .expect("spawned self-wait task stays alive");
                seen.lock().push((operation, observation));
            }
            Ok(())
        }
    }
}

#[tokio::test]
async fn attributed_spawn_from_apply_refuses_every_lifecycle_self_wait() {
    for operation in OPERATIONS {
        let ctx = Context::new();
        let fiber_handle_cell = Arc::new(Mutex::new(None));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let fiber_handle = ctx
            .spawn(PreparedPlugin::from_input(
                SpawnedApplyProbe {
                    fiber_handle: fiber_handle_cell.clone(),
                    seen: seen.clone(),
                },
                operation,
            ))
            .await
            .unwrap();
        *fiber_handle_cell.lock() = Some(fiber_handle.clone());
        let id = fiber_handle.id();

        bounded(2_000, fiber_handle.restart())
            .await
            .expect("attributed spawned self-wait must refuse instead of deadlocking")
            .unwrap_or_else(|error| panic!("host restart for {operation:?} failed: {error:?}"));

        let (reported_operation, observation) = {
            let mut outcomes = seen.lock();
            assert_eq!(
                outcomes.len(),
                1,
                "one second-apply probe for {operation:?}"
            );
            outcomes.pop().unwrap()
        };
        assert_eq!(reported_operation, operation);
        assert_recursion(observation, operation, &id);
        assert_eq!(fiber_handle.state(), FiberState::Active);
        fiber_handle.dispose().await.unwrap();
    }
}

struct EarlyDisposeProbe {
    seen: Arc<Mutex<Option<LifecycleRecursion>>>,
}

impl Plugin for EarlyDisposeProbe {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn apply(
        &self,
        ctx: Context,
        _prepared: &(),
    ) -> impl Future<Output = Result<(), Infallible>> + Send {
        let cleanup_ctx = ctx.clone();
        let seen = self.seen.clone();
        async move {
            let registration = ctx
                .effect(move || async move {
                    let recursion = cleanup_ctx
                        .remove_plugins::<EarlyDisposeProbe>()
                        .await
                        .expect_err("manual-dispose cleanup must retain apply attribution");
                    *seen.lock() = Some(recursion);
                })
                .expect("apply generation is open to cleanup");

            assert!(
                registration
                    .dispose()
                    .await
                    .expect("manual cleanup itself succeeds"),
                "apply still owns the exact cleanup occurrence"
            );
            Ok(())
        }
    }
}

#[tokio::test]
async fn manual_dispose_from_apply_keeps_settle_attribution_across_cleanup_task() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(None));

    let fiber_handle = bounded(
        1_000,
        ctx.spawn(PreparedPlugin::from_input(
            EarlyDisposeProbe { seen: seen.clone() },
            (),
        )),
    )
    .await
    .expect("manual dispose must refuse recursive group removal instead of deadlocking")
    .expect("probe spawn succeeds after typed recursion refusal");

    let recursion = seen
        .lock()
        .take()
        .expect("cleanup observed typed lifecycle recursion");
    assert_eq!(recursion.operation(), LifecycleOperation::RemovePlugins);
    let id = fiber_handle.id();
    assert_eq!(recursion.fiber_id(), &id);
    fiber_handle.dispose().await.unwrap();
}

struct ExternalManualDisposeProbe {
    registration: Arc<Mutex<Option<EffectRegistration>>>,
    removed: Arc<AtomicBool>,
}

impl Plugin for ExternalManualDisposeProbe {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn apply(
        &self,
        ctx: Context,
        _prepared: &(),
    ) -> impl Future<Output = Result<(), Infallible>> + Send {
        let cleanup_ctx = ctx.clone();
        let registration = self.registration.clone();
        let removed = self.removed.clone();
        async move {
            let effect = ctx
                .effect(move || async move {
                    cleanup_ctx
                        .remove_plugins::<ExternalManualDisposeProbe>()
                        .await
                        .expect("external manual cleanup has no settle recursion");
                    removed.store(true, Ordering::SeqCst);
                })
                .expect("apply generation is open to cleanup");
            *registration.lock() = Some(effect);
            Ok(())
        }
    }
}

#[tokio::test]
async fn external_manual_dispose_does_not_invent_settle_attribution() {
    let ctx = Context::new();
    let registration = Arc::new(Mutex::new(None));
    let removed = Arc::new(AtomicBool::new(false));
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(
            ExternalManualDisposeProbe {
                registration: registration.clone(),
                removed: removed.clone(),
            },
            (),
        ))
        .await
        .unwrap();

    let effect = registration
        .lock()
        .take()
        .expect("apply published its manual cleanup control");
    let claimed = bounded(2_000, effect.dispose())
        .await
        .expect("external manual cleanup must not deadlock")
        .expect("external manual cleanup succeeds");
    assert!(claimed);
    assert!(removed.load(Ordering::SeqCst));
    assert_eq!(fiber_handle.state(), FiberState::Disposed);
}

#[tokio::test]
async fn own_apply_refuses_every_lifecycle_self_wait_by_exact_allocation() {
    for operation in OPERATIONS {
        let ctx = Context::new();
        let fiber_handle_cell = Arc::new(Mutex::new(None));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let fiber_handle = ctx
            .spawn(PreparedPlugin::from_input(
                ApplyProbe {
                    fiber_handle: fiber_handle_cell.clone(),
                    seen: seen.clone(),
                },
                operation,
            ))
            .await
            .unwrap();
        *fiber_handle_cell.lock() = Some(fiber_handle.clone());
        let id = fiber_handle.id();

        fiber_handle
            .restart()
            .await
            .unwrap_or_else(|error| panic!("host restart for {operation:?} failed: {error:?}"));

        let (reported_operation, observation) = {
            let mut outcomes = seen.lock();
            assert_eq!(
                outcomes.len(),
                1,
                "one second-apply probe for {operation:?}"
            );
            outcomes.pop().unwrap()
        };
        assert_eq!(reported_operation, operation);
        assert_recursion(observation, operation, &id);
        assert_eq!(fiber_handle.id(), id);
        assert_eq!(fiber_handle.state(), FiberState::Active);
        fiber_handle.dispose().await.unwrap();
    }
}

// ---------------------------------------------------------------------------
// Disposer attribution: terminal death must not turn recursion into Closed.
// ---------------------------------------------------------------------------

struct CleanupProbe {
    fiber_handle: Arc<Mutex<Option<FiberHandle>>>,
    seen: Arc<Mutex<Vec<(LifecycleOperation, Observation)>>>,
}

impl Plugin for CleanupProbe {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn apply(
        &self,
        ctx: Context,
        _prepared: &(),
    ) -> impl Future<Output = Result<(), Infallible>> + Send {
        let fiber_handle = self.fiber_handle.clone();
        let seen = self.seen.clone();
        let cleanup_ctx = ctx.clone();
        async move {
            ctx.effect(move || async move {
                let fiber_handle = fiber_handle
                    .lock()
                    .clone()
                    .expect("FiberHandle is installed before terminal cleanup");
                for operation in OPERATIONS {
                    let observation =
                        observe::<CleanupProbe, _>(&cleanup_ctx, &fiber_handle, operation, || ())
                            .await;
                    seen.lock().push((operation, observation));
                }
            })
            .unwrap();
            Ok(())
        }
    }
}

#[tokio::test]
async fn terminal_disposer_keeps_exact_allocation_attribution_after_lifecycle_death() {
    let ctx = Context::new();
    let fiber_handle_cell = Arc::new(Mutex::new(None));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(
            CleanupProbe {
                fiber_handle: fiber_handle_cell.clone(),
                seen: seen.clone(),
            },
            (),
        ))
        .await
        .unwrap();
    *fiber_handle_cell.lock() = Some(fiber_handle.clone());
    let id = fiber_handle.id();

    bounded(7_000, fiber_handle.dispose())
        .await
        .expect("terminal disposal must complete")
        .unwrap();
    assert_eq!(fiber_handle.state(), FiberState::Disposed);

    let outcomes = std::mem::take(&mut *seen.lock());
    assert_eq!(outcomes.len(), OPERATIONS.len());
    for (operation, observation) in outcomes {
        assert_recursion(observation, operation, &id);
    }
}

#[tokio::test]
async fn reload_disposer_refuses_every_lifecycle_self_wait_by_exact_allocation() {
    let ctx = Context::new();
    let fiber_handle_cell = Arc::new(Mutex::new(None));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(
            CleanupProbe {
                fiber_handle: fiber_handle_cell.clone(),
                seen: seen.clone(),
            },
            (),
        ))
        .await
        .unwrap();
    *fiber_handle_cell.lock() = Some(fiber_handle.clone());
    let id = fiber_handle.id();

    bounded(7_000, fiber_handle.restart())
        .await
        .expect("reload must complete")
        .unwrap();
    assert_eq!(fiber_handle.state(), FiberState::Active);

    let outcomes = std::mem::take(&mut *seen.lock());
    assert_eq!(outcomes.len(), OPERATIONS.len());
    for (operation, observation) in outcomes {
        assert_recursion(observation, operation, &id);
    }
}

// ---------------------------------------------------------------------------
// Context::run attribution: mutators always refuse; observers only in drain.
// ---------------------------------------------------------------------------

struct RunProbe {
    fiber_handle: Arc<Mutex<Option<FiberHandle>>>,
    start: Arc<Notify>,
    seen: Arc<Mutex<Vec<(LifecycleOperation, Observation)>>>,
    done: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
}

impl Plugin for RunProbe {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn apply(
        &self,
        ctx: Context,
        _prepared: &(),
    ) -> impl Future<Output = Result<(), Infallible>> + Send {
        let fiber_handle = self.fiber_handle.clone();
        let start = self.start.clone();
        let seen = self.seen.clone();
        let done = self.done.lock().take();
        async move {
            if let Some(done) = done {
                let task_ctx = ctx.clone();
                ctx.run(async move {
                    start.notified().await;
                    let fiber_handle = fiber_handle
                        .lock()
                        .clone()
                        .expect("FiberHandle installed before task starts");
                    for operation in [LifecycleOperation::Ready, LifecycleOperation::WaitState] {
                        let observation =
                            observe::<RunProbe, _>(&task_ctx, &fiber_handle, operation, || ())
                                .await;
                        seen.lock().push((operation, observation));
                    }
                    for operation in MUTATING_OPERATIONS {
                        let observation =
                            observe::<RunProbe, _>(&task_ctx, &fiber_handle, operation, || ())
                                .await;
                        seen.lock().push((operation, observation));
                    }
                    let _ = done.send(());
                })
                .unwrap();
            }
            Ok(())
        }
    }
}

#[tokio::test]
async fn run_task_refuses_mutators_but_allows_idle_observers() {
    let ctx = Context::new();
    let fiber_handle_cell = Arc::new(Mutex::new(None));
    let start = Arc::new(Notify::new());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(
            RunProbe {
                fiber_handle: fiber_handle_cell.clone(),
                start: start.clone(),
                seen: seen.clone(),
                done: Arc::new(Mutex::new(Some(done_tx))),
            },
            (),
        ))
        .await
        .unwrap();
    *fiber_handle_cell.lock() = Some(fiber_handle.clone());
    let id = fiber_handle.id();
    start.notify_one();
    bounded(4_000, done_rx)
        .await
        .expect("run task must finish")
        .expect("run task completion sender stays alive");

    let outcomes = std::mem::take(&mut *seen.lock());
    assert_eq!(outcomes.len(), OPERATIONS.len());
    for (operation, observation) in outcomes {
        if matches!(
            operation,
            LifecycleOperation::Ready | LifecycleOperation::WaitState
        ) {
            assert_success(observation, operation);
        } else {
            assert_recursion(observation, operation, &id);
        }
    }
    fiber_handle.dispose().await.unwrap();
}

struct DrainObserverProbe {
    fiber_handle: Arc<Mutex<Option<FiberHandle>>>,
    seen: Arc<Mutex<Vec<(LifecycleOperation, Observation)>>>,
}

impl Plugin for DrainObserverProbe {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn apply(
        &self,
        ctx: Context,
        _prepared: &(),
    ) -> impl Future<Output = Result<(), Infallible>> + Send {
        let fiber_handle = self.fiber_handle.clone();
        let seen = self.seen.clone();
        async move {
            let (signal_tx, signal_rx) = tokio::sync::oneshot::channel();
            let task_ctx = ctx.clone();
            ctx.run(async move {
                signal_rx.await.expect("cleanup signals the run task once");
                let fiber_handle = fiber_handle
                    .lock()
                    .clone()
                    .expect("FiberHandle installed before drain");
                for operation in [LifecycleOperation::Ready, LifecycleOperation::WaitState] {
                    let observation = observe::<DrainObserverProbe, _>(
                        &task_ctx,
                        &fiber_handle,
                        operation,
                        || (),
                    )
                    .await;
                    seen.lock().push((operation, observation));
                }
            })
            .unwrap();

            let signal = Arc::new(Mutex::new(Some(signal_tx)));
            ctx.effect(move || async move {
                if let Some(signal) = signal.lock().take() {
                    let _ = signal.send(());
                }
            })
            .unwrap();
            Ok(())
        }
    }
}

#[tokio::test]
async fn run_task_observers_refuse_only_while_the_target_drain_joins_them() {
    let ctx = Context::new();
    let fiber_handle_cell = Arc::new(Mutex::new(None));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(
            DrainObserverProbe {
                fiber_handle: fiber_handle_cell.clone(),
                seen: seen.clone(),
            },
            (),
        ))
        .await
        .unwrap();
    *fiber_handle_cell.lock() = Some(fiber_handle.clone());
    let id = fiber_handle.id();

    bounded(4_000, fiber_handle.dispose())
        .await
        .expect("observer refusal lets drain join complete")
        .unwrap();
    let outcomes = std::mem::take(&mut *seen.lock());
    assert_eq!(outcomes.len(), 2);
    for (operation, observation) in outcomes {
        assert_recursion(observation, operation, &id);
    }
}

// ---------------------------------------------------------------------------
// Negative discrimination: unrelated tasks/Fibers remain legal.
// ---------------------------------------------------------------------------

struct BlockingRestart {
    applies: Arc<AtomicUsize>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

impl Plugin for BlockingRestart {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, _ctx: Context, _prepared: &()) -> Result<(), Infallible> {
        let round = self.applies.fetch_add(1, Ordering::SeqCst) + 1;
        if round == 2 {
            self.entered.notify_one();
            self.release.notified().await;
        }
        Ok(())
    }
}

#[tokio::test]
async fn unrelated_task_restart_waits_out_an_inflight_pass_instead_of_over_refusing() {
    let ctx = Context::new();
    let applies = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(
            BlockingRestart {
                applies: applies.clone(),
                entered: entered.clone(),
                release: release.clone(),
            },
            (),
        ))
        .await
        .unwrap();

    let first = tokio::spawn({
        let fiber_handle = fiber_handle.clone();
        async move { fiber_handle.restart().await }
    });
    entered.notified().await;
    let second = tokio::spawn({
        let fiber_handle = fiber_handle.clone();
        async move { fiber_handle.restart().await }
    });
    release.notify_one();

    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    assert_eq!(applies.load(Ordering::SeqCst), 3);
    assert_eq!(fiber_handle.ready().await.unwrap(), FiberState::Active);
}

#[derive(Clone)]
struct CrossTarget {
    seen: Arc<Mutex<Vec<u8>>>,
}
impl Plugin for CrossTarget {
    type Config = u8;
    type Input = u8;
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, value: u8) -> Result<u8, Infallible> {
        Ok(value)
    }
    async fn apply(&self, _ctx: Context, value: &u8) -> Result<(), Infallible> {
        self.seen.lock().push(*value);
        Ok(())
    }
}

struct CrossCaller {
    target: FiberHandle,
    successor: Arc<Mutex<Option<FiberHandle>>>,
    completed: Arc<AtomicBool>,
}
impl Plugin for CrossCaller {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, _ctx: Context, _prepared: &()) -> Result<(), Infallible> {
        assert_eq!(self.target.ready().await.unwrap(), FiberState::Active);
        self.target
            .update(PreparedChange::from_input::<CrossTarget>(2))
            .await
            .expect("a different Fiber's apply may update the target");
        let successor = self
            .target
            .era_swap(PreparedChange::from_input::<CrossTarget>(3))
            .await
            .expect("a different Fiber's apply may era-swap the target");
        *self.successor.lock() = Some(successor);
        self.completed.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn unrelated_fiber_apply_may_ready_update_and_era_swap_another_allocation() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let target = ctx
        .spawn(PreparedPlugin::from_input(
            CrossTarget { seen: seen.clone() },
            1,
        ))
        .await
        .unwrap();
    let target_id = target.id();
    let completed = Arc::new(AtomicBool::new(false));
    let successor = Arc::new(Mutex::new(None));
    let caller = ctx
        .spawn(PreparedPlugin::from_input(
            CrossCaller {
                target: target.clone(),
                successor: successor.clone(),
                completed: completed.clone(),
            },
            (),
        ))
        .await
        .unwrap();

    assert!(completed.load(Ordering::SeqCst));
    assert_eq!(target.id(), target_id);
    assert_eq!(target.state(), FiberState::Disposed);
    let successor = successor
        .lock()
        .clone()
        .expect("era swap produced successor");
    assert_ne!(successor.id(), target_id);
    assert_eq!(successor.state(), FiberState::Active);
    assert_eq!(*seen.lock(), vec![1, 2, 3]);
    assert_eq!(caller.state(), FiberState::Active);
}
