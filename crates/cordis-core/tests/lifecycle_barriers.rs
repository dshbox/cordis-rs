//! Issue 29 evidence for restart, terminal disposal, and permanent-root barriers.

mod common;

use cordis_core::lifecycle::{LifecycleOperation, PluginFailureKind, RestartError};
use cordis_core::{
    Context, FiberState, InjectSpec, Plugin, PreparedChange, PreparedPlugin, Service, UpdateOutcome,
};
use parking_lot::Mutex;
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::task::{Context as TaskContext, Poll};

#[derive(Debug, thiserror::Error)]
#[error("restart apply failed")]
struct ApplyBoom;

fn prepared<P>(plugin: P) -> PreparedPlugin
where
    P: Plugin<Config = (), Input = (), PrepareError = Infallible>,
{
    PreparedPlugin::from_input(plugin, ())
}

struct RetrySameTarget {
    applies: Arc<AtomicU32>,
}

impl Plugin for RetrySameTarget {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = ApplyBoom;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, _ctx: Context, _prepared: &()) -> Result<(), ApplyBoom> {
        let attempt = self.applies.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt == 2 { Err(ApplyBoom) } else { Ok(()) }
    }
}

#[tokio::test]
async fn restart_preserves_identity_and_retries_a_same_target_failure() {
    let root = Context::new();
    let applies = Arc::new(AtomicU32::new(0));
    let fiber_handle = root
        .spawn(prepared(RetrySameTarget {
            applies: applies.clone(),
        }))
        .await
        .unwrap();
    let id = fiber_handle.id();

    let error = fiber_handle
        .restart()
        .await
        .expect_err("second apply fails");
    let RestartError::Apply(failure) = error else {
        panic!("expected typed apply failure: {error:?}")
    };
    assert_eq!(failure.kind(), PluginFailureKind::ReturnedError);
    assert_eq!(fiber_handle.state(), FiberState::Failed);
    assert_eq!(fiber_handle.id(), id);

    fiber_handle
        .restart()
        .await
        .expect("explicit restart retries the same parked target");
    assert_eq!(fiber_handle.state(), FiberState::Active);
    assert_eq!(fiber_handle.id(), id);
    assert_eq!(applies.load(Ordering::SeqCst), 3);
}

struct PanicOnRestart {
    applies: Arc<AtomicU32>,
}

impl Plugin for PanicOnRestart {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, _ctx: Context, _prepared: &()) -> Result<(), Infallible> {
        let attempt = self.applies.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt == 2 {
            panic!("restart panic")
        }
        Ok(())
    }
}

#[tokio::test]
async fn restart_contains_apply_panic_after_complete_rollback() {
    let root = Context::new();
    let applies = Arc::new(AtomicU32::new(0));
    let fiber_handle = root
        .spawn(prepared(PanicOnRestart {
            applies: applies.clone(),
        }))
        .await
        .unwrap();
    let id = fiber_handle.id();

    let error = fiber_handle
        .restart()
        .await
        .expect_err("second apply panics");
    let RestartError::Apply(failure) = error else {
        panic!("expected normalized restart panic: {error:?}")
    };
    assert_eq!(failure.kind(), PluginFailureKind::Panic);
    assert!(failure.diagnostic().contains("restart panic"));
    assert_eq!(fiber_handle.state(), FiberState::Failed);
    assert_eq!(fiber_handle.id(), id);
    assert_eq!(applies.load(Ordering::SeqCst), 2);
}

#[derive(Debug)]
struct Missing;
impl Service for Missing {
    const NAME: &'static str = "t29-missing";
}

struct NeedsMissing {
    applies: Arc<AtomicU32>,
}
impl Plugin for NeedsMissing {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = ApplyBoom;
    fn inject(&self) -> InjectSpec {
        InjectSpec::none().require(Missing::NAME)
    }
    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, _ctx: Context, _prepared: &()) -> Result<(), ApplyBoom> {
        self.applies.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn restart_keeps_missing_requirements_pending_without_apply_and_closed_refuses() {
    let root = Context::new();
    let applies = Arc::new(AtomicU32::new(0));
    let fiber_handle = root
        .spawn(prepared(NeedsMissing {
            applies: applies.clone(),
        }))
        .await
        .unwrap();
    assert_eq!(fiber_handle.state(), FiberState::Pending);
    fiber_handle.restart().await.unwrap();
    assert_eq!(fiber_handle.state(), FiberState::Pending);
    assert_eq!(applies.load(Ordering::SeqCst), 0);

    fiber_handle.dispose().await.unwrap();
    let error = fiber_handle
        .restart()
        .await
        .expect_err("closed Fiber refuses before change");
    assert!(matches!(error, RestartError::Closed));
    assert_eq!(fiber_handle.state(), FiberState::Disposed);
}

struct RecursingCleanup {
    fiber_handle: Arc<Mutex<Option<cordis_core::FiberHandle>>>,
    restart_recursion: Arc<Mutex<Option<(LifecycleOperation, cordis_core::FiberId)>>>,
    dispose_recursion: Arc<Mutex<Option<(LifecycleOperation, cordis_core::FiberId)>>>,
}

impl Plugin for RecursingCleanup {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = ApplyBoom;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), ApplyBoom> {
        let fiber_handle_for_dispose = self.fiber_handle.clone();
        let dispose_recursion = self.dispose_recursion.clone();
        ctx.effect(move || async move {
            let fiber_handle = fiber_handle_for_dispose.lock().clone().unwrap();
            let recursion = fiber_handle
                .dispose()
                .await
                .expect_err("self-dispose must refuse");
            *dispose_recursion.lock() = Some((recursion.operation(), recursion.fiber_id().clone()));
        })
        .map_err(|_| ApplyBoom)?;

        let fiber_handle_for_restart = self.fiber_handle.clone();
        let restart_recursion = self.restart_recursion.clone();
        ctx.effect(move || async move {
            let fiber_handle = fiber_handle_for_restart.lock().clone().unwrap();
            let error = fiber_handle
                .restart()
                .await
                .expect_err("self-restart must refuse");
            let RestartError::Recursion(recursion) = error else {
                panic!("expected typed restart recursion, got {error:?}");
            };
            *restart_recursion.lock() = Some((recursion.operation(), recursion.fiber_id().clone()));
        })
        .map_err(|_| ApplyBoom)?;
        Ok(())
    }
}

#[tokio::test]
async fn restart_and_dispose_refuse_self_waits_with_typed_operation_and_fiber_identity() {
    let root = Context::new();
    let fiber_handle_cell = Arc::new(Mutex::new(None));
    let restart_recursion = Arc::new(Mutex::new(None));
    let dispose_recursion = Arc::new(Mutex::new(None));
    let fiber_handle = root
        .spawn(prepared(RecursingCleanup {
            fiber_handle: fiber_handle_cell.clone(),
            restart_recursion: restart_recursion.clone(),
            dispose_recursion: dispose_recursion.clone(),
        }))
        .await
        .unwrap();
    *fiber_handle_cell.lock() = Some(fiber_handle.clone());
    let id = fiber_handle.id();

    fiber_handle.restart().await.unwrap();

    assert_eq!(
        restart_recursion.lock().as_ref(),
        Some(&(LifecycleOperation::Restart, id.clone()))
    );
    assert_eq!(
        dispose_recursion.lock().as_ref(),
        Some(&(LifecycleOperation::Dispose, id))
    );
}

struct RestartCancellation {
    applies: Arc<AtomicU32>,
    cleanup_started: Arc<tokio::sync::Notify>,
    cleanup_release: Arc<tokio::sync::Notify>,
}
impl Plugin for RestartCancellation {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = ApplyBoom;
    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), ApplyBoom> {
        let attempt = self.applies.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt == 1 {
            let started = self.cleanup_started.clone();
            let release = self.cleanup_release.clone();
            ctx.effect(move || async move {
                started.notify_one();
                release.notified().await;
            })
            .map_err(|_| ApplyBoom)?;
        }
        Ok(())
    }
}

#[tokio::test]
async fn cancelled_precommit_restart_waiter_does_not_replace_the_generation() {
    let root = Context::new();
    let applies = Arc::new(AtomicU32::new(0));
    let cleanup_started = Arc::new(tokio::sync::Notify::new());
    let cleanup_release = Arc::new(tokio::sync::Notify::new());
    let fiber_handle = root
        .spawn(prepared(RestartCancellation {
            applies: applies.clone(),
            cleanup_started: cleanup_started.clone(),
            cleanup_release: cleanup_release.clone(),
        }))
        .await
        .unwrap();

    let first = tokio::spawn({
        let fiber_handle = fiber_handle.clone();
        async move { fiber_handle.restart().await }
    });
    cleanup_started.notified().await;
    assert_eq!(fiber_handle.state(), FiberState::Unloading);

    let mut cancelled = Box::pin(fiber_handle.restart());
    assert!(
        futures::poll!(&mut cancelled).is_pending(),
        "second restart must still be waiting before its generation-replacement commit"
    );
    drop(cancelled);

    cleanup_release.notify_one();
    first.await.unwrap().unwrap();
    assert_eq!(fiber_handle.state(), FiberState::Active);
    assert_eq!(
        applies.load(Ordering::SeqCst),
        2,
        "cancelling before the second restart commit must not replace another generation"
    );

    fiber_handle
        .restart()
        .await
        .expect("a later restart still acquires the lifecycle slot");
    assert_eq!(applies.load(Ordering::SeqCst), 3);
}

#[test]
fn committed_restart_survives_origin_runtime_shutdown() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let root = Context::new();
    let applies = Arc::new(AtomicU32::new(0));
    let cleanup_started = Arc::new(tokio::sync::Notify::new());
    let cleanup_release = Arc::new(tokio::sync::Notify::new());
    let fiber_handle = runtime
        .block_on(root.spawn(prepared(RestartCancellation {
            applies: applies.clone(),
            cleanup_started: cleanup_started.clone(),
            cleanup_release: cleanup_release.clone(),
        })))
        .unwrap();

    let handle = runtime.handle().clone();
    let restarting = fiber_handle.clone();
    let restart = std::thread::spawn(move || handle.block_on(restarting.restart()));

    runtime.block_on(cleanup_started.notified());
    assert_eq!(fiber_handle.state(), FiberState::Unloading);
    runtime.shutdown_background();
    cleanup_release.notify_one();

    restart
        .join()
        .expect("runtime shutdown must not panic a committed restart")
        .expect("committed restart still reaches current-target quiescence");
    assert_eq!(fiber_handle.state(), FiberState::Active);
    assert_eq!(applies.load(Ordering::SeqCst), 2);
}

// Poll a real Tokio driver-bound resource to Pending before publishing the
// exact shutdown barrier. This distinguishes task transfer from terminal
// completion without relying on scheduler timing or repeated stress runs.
struct RuntimeBoundPollProbe {
    timer: Option<Pin<Box<tokio::time::Sleep>>>,
    armed: Option<std::sync::mpsc::Sender<()>>,
}

impl RuntimeBoundPollProbe {
    fn new(armed: std::sync::mpsc::Sender<()>) -> Self {
        Self {
            timer: None,
            armed: Some(armed),
        }
    }
}

impl Future for RuntimeBoundPollProbe {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<()> {
        let this = self.as_mut().get_mut();
        let timer = this.timer.get_or_insert_with(|| {
            Box::pin(tokio::time::sleep(std::time::Duration::from_millis(200)))
        });
        let outcome = timer.as_mut().poll(cx);
        if outcome.is_pending()
            && let Some(armed) = this.armed.take()
        {
            armed.send(()).unwrap();
        }
        outcome
    }
}

struct RuntimeBoundReplacement {
    applies: Arc<AtomicU32>,
    armed: parking_lot::Mutex<Option<std::sync::mpsc::Sender<()>>>,
}

impl Plugin for RuntimeBoundReplacement {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, _ctx: Context, _prepared: &()) -> Result<(), Infallible> {
        if self.applies.fetch_add(1, Ordering::SeqCst) == 1 {
            let armed = self.armed.lock().take().expect("replacement applies once");
            RuntimeBoundPollProbe::new(armed).await;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct RuntimeAffinityService;
impl Service for RuntimeAffinityService {
    const NAME: &'static str = "runtime-affinity-service";
}

struct RuntimeBoundDependent {
    armed: parking_lot::Mutex<Option<std::sync::mpsc::Sender<()>>>,
}

impl Plugin for RuntimeBoundDependent {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn inject(&self) -> InjectSpec {
        InjectSpec::none().require(RuntimeAffinityService::NAME)
    }

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, _ctx: Context, _prepared: &()) -> Result<(), Infallible> {
        let armed = self.armed.lock().take().expect("dependent applies once");
        RuntimeBoundPollProbe::new(armed).await;
        Ok(())
    }
}

#[test]
fn dependent_convergence_polls_user_future_only_on_completion_runtime() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let root = Context::new();
    let (armed_tx, armed_rx) = std::sync::mpsc::channel();
    let dependent = runtime
        .block_on(root.spawn(prepared(RuntimeBoundDependent {
            armed: parking_lot::Mutex::new(Some(armed_tx)),
        })))
        .unwrap();
    assert_eq!(dependent.state(), FiberState::Pending);

    let publication = runtime
        .block_on(async { root.provide(Arc::new(RuntimeAffinityService)) })
        .unwrap();
    armed_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("dependent apply polled its completion-runtime timer to Pending");
    runtime.shutdown_background();

    let observer = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert_eq!(
        observer
            .block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(2), dependent.ready()).await
            })
            .expect("dependent convergence survives origin runtime shutdown")
            .unwrap(),
        FiberState::Active
    );
    observer.block_on(dependent.dispose()).unwrap();
    drop(publication);
}

fn run_runtime_bound_replacement<T, F, Fut>(
    operation: F,
) -> (
    std::thread::Result<T>,
    cordis_core::FiberHandle,
    Arc<AtomicU32>,
)
where
    T: Send + 'static,
    F: FnOnce(cordis_core::FiberHandle) -> Fut + Send + 'static,
    Fut: Future<Output = T> + Send + 'static,
{
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let root = Context::new();
    let applies = Arc::new(AtomicU32::new(0));
    let (armed_tx, armed_rx) = std::sync::mpsc::channel();
    let fiber_handle = runtime
        .block_on(root.spawn(prepared(RuntimeBoundReplacement {
            applies: applies.clone(),
            armed: parking_lot::Mutex::new(Some(armed_tx)),
        })))
        .unwrap();

    let handle = runtime.handle().clone();
    let running = fiber_handle.clone();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            handle.block_on(operation(running))
        }));
        let _ = result_tx.send(result);
    });

    armed_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("replacement apply polled its origin-runtime timer to Pending");
    runtime.shutdown_background();

    let operation = result_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("committed lifecycle operation publishes its result");
    worker.join().unwrap();
    (operation, fiber_handle, applies)
}

fn assert_runtime_bound_replacement_ready(fiber_handle: &cordis_core::FiberHandle) {
    let observer = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert_eq!(
        observer
            .block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(2), fiber_handle.ready()).await
            })
            .expect("ready reaches the committed replacement terminal barrier")
            .unwrap(),
        FiberState::Active
    );
}

#[test]
fn committed_restart_does_not_migrate_a_polled_user_future_between_runtimes() {
    let (operation, fiber_handle, applies) =
        run_runtime_bound_replacement(|fiber_handle| async move { fiber_handle.restart().await });
    operation
        .expect("committed restart task does not panic after executor handoff")
        .expect("origin runtime shutdown does not become a user apply failure");
    assert_runtime_bound_replacement_ready(&fiber_handle);
    assert_eq!(applies.load(Ordering::SeqCst), 2);
}

#[test]
fn committed_update_does_not_migrate_a_polled_user_future_between_runtimes() {
    let (operation, fiber_handle, applies) =
        run_runtime_bound_replacement(|fiber_handle| async move {
            fiber_handle
                .update(PreparedChange::from_input::<RuntimeBoundReplacement>(()))
                .await
        });
    assert_eq!(
        operation
            .expect("committed update task does not panic after executor handoff")
            .expect("origin runtime shutdown does not become a user apply failure"),
        UpdateOutcome::Committed(FiberState::Active)
    );
    assert_runtime_bound_replacement_ready(&fiber_handle);
    assert_eq!(applies.load(Ordering::SeqCst), 2);
}

#[test]
fn committed_era_swap_does_not_migrate_a_polled_user_future_between_runtimes() {
    let (operation, source, applies) = run_runtime_bound_replacement(|fiber_handle| async move {
        fiber_handle
            .era_swap(PreparedChange::from_input::<RuntimeBoundReplacement>(()))
            .await
    });
    let successor = operation
        .expect("committed era-swap task does not panic after executor handoff")
        .expect("origin runtime shutdown does not become a successor apply failure");
    assert_eq!(source.state(), FiberState::Disposed);
    assert_runtime_bound_replacement_ready(&successor);
    assert_eq!(applies.load(Ordering::SeqCst), 2);
}

#[test]
fn committed_update_survives_origin_runtime_shutdown() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let root = Context::new();
    let applies = Arc::new(AtomicU32::new(0));
    let cleanup_started = Arc::new(tokio::sync::Notify::new());
    let cleanup_release = Arc::new(tokio::sync::Notify::new());
    let fiber_handle = runtime
        .block_on(root.spawn(prepared(RestartCancellation {
            applies: applies.clone(),
            cleanup_started: cleanup_started.clone(),
            cleanup_release: cleanup_release.clone(),
        })))
        .unwrap();

    let handle = runtime.handle().clone();
    let updating = fiber_handle.clone();
    let update = std::thread::spawn(move || {
        handle.block_on(updating.update(PreparedChange::from_input::<RestartCancellation>(())))
    });

    runtime.block_on(cleanup_started.notified());
    assert_eq!(fiber_handle.state(), FiberState::Unloading);
    runtime.shutdown_background();
    cleanup_release.notify_one();

    assert_eq!(
        update
            .join()
            .expect("runtime shutdown must not panic a committed update")
            .expect("committed update still reaches current-target quiescence"),
        UpdateOutcome::Committed(FiberState::Active)
    );
    assert_eq!(fiber_handle.state(), FiberState::Active);
    assert_eq!(applies.load(Ordering::SeqCst), 2);
}

#[test]
fn committed_era_swap_survives_origin_runtime_shutdown() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let root = Context::new();
    let applies = Arc::new(AtomicU32::new(0));
    let cleanup_started = Arc::new(tokio::sync::Notify::new());
    let cleanup_release = Arc::new(tokio::sync::Notify::new());
    let source = runtime
        .block_on(root.spawn(prepared(RestartCancellation {
            applies: applies.clone(),
            cleanup_started: cleanup_started.clone(),
            cleanup_release: cleanup_release.clone(),
        })))
        .unwrap();
    let source_id = source.id().clone();

    let handle = runtime.handle().clone();
    let swapping = source.clone();
    let swap = std::thread::spawn(move || {
        handle.block_on(swapping.era_swap(PreparedChange::from_input::<RestartCancellation>(())))
    });

    runtime.block_on(cleanup_started.notified());
    assert_eq!(source.state(), FiberState::Unloading);
    runtime.shutdown_background();
    cleanup_release.notify_one();

    let successor = swap
        .join()
        .expect("runtime shutdown must not panic a committed era swap")
        .expect("committed era swap still completes successor handoff");
    assert_eq!(source.state(), FiberState::Disposed);
    assert_eq!(successor.state(), FiberState::Active);
    assert_ne!(successor.id(), source_id);
    assert_eq!(applies.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn cancelled_precommit_dispose_waiter_leaves_the_fiber_live() {
    let root = Context::new();
    let applies = Arc::new(AtomicU32::new(0));
    let cleanup_started = Arc::new(tokio::sync::Notify::new());
    let cleanup_release = Arc::new(tokio::sync::Notify::new());
    let fiber_handle = root
        .spawn(prepared(RestartCancellation {
            applies: applies.clone(),
            cleanup_started: cleanup_started.clone(),
            cleanup_release: cleanup_release.clone(),
        }))
        .await
        .unwrap();

    let restart = tokio::spawn({
        let fiber_handle = fiber_handle.clone();
        async move { fiber_handle.restart().await }
    });
    cleanup_started.notified().await;

    let mut dispose = Box::pin(fiber_handle.dispose());
    assert!(
        futures::poll!(&mut dispose).is_pending(),
        "dispose must still be waiting before its Open-to-Closing claim"
    );
    drop(dispose);

    cleanup_release.notify_one();
    restart.await.unwrap().unwrap();
    assert_eq!(fiber_handle.state(), FiberState::Active);
    assert_eq!(applies.load(Ordering::SeqCst), 2);

    fiber_handle
        .dispose()
        .await
        .expect("a later disposer can still win the terminal claim");
    assert_eq!(fiber_handle.state(), FiberState::Disposed);
}

#[tokio::test]
async fn cancelled_postcommit_restart_waiter_does_not_stop_the_restart() {
    let root = Context::new();
    let applies = Arc::new(AtomicU32::new(0));
    let cleanup_started = Arc::new(tokio::sync::Notify::new());
    let cleanup_release = Arc::new(tokio::sync::Notify::new());
    let fiber_handle = root
        .spawn(prepared(RestartCancellation {
            applies: applies.clone(),
            cleanup_started: cleanup_started.clone(),
            cleanup_release: cleanup_release.clone(),
        }))
        .await
        .unwrap();
    let id = fiber_handle.id();

    let waiter = tokio::spawn({
        let fiber_handle = fiber_handle.clone();
        async move { fiber_handle.restart().await }
    });
    cleanup_started.notified().await;
    assert_eq!(fiber_handle.state(), FiberState::Unloading);
    waiter.abort();
    cleanup_release.notify_one();

    assert_eq!(fiber_handle.ready().await.unwrap(), FiberState::Active);
    assert_eq!(fiber_handle.id(), id);
    assert_eq!(applies.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn committed_restart_serializes_terminal_dispose_behind_its_barrier() {
    let root = Context::new();
    let applies = Arc::new(AtomicU32::new(0));
    let cleanup_started = Arc::new(tokio::sync::Notify::new());
    let cleanup_release = Arc::new(tokio::sync::Notify::new());
    let fiber_handle = root
        .spawn(prepared(RestartCancellation {
            applies: applies.clone(),
            cleanup_started: cleanup_started.clone(),
            cleanup_release: cleanup_release.clone(),
        }))
        .await
        .unwrap();

    let restart = tokio::spawn({
        let fiber_handle = fiber_handle.clone();
        async move { fiber_handle.restart().await }
    });
    cleanup_started.notified().await;

    let mut dispose = Box::pin(fiber_handle.dispose());
    assert!(
        futures::poll!(&mut dispose).is_pending(),
        "terminal dispose must not pass a committed restart that still owns the lifecycle slot"
    );

    cleanup_release.notify_one();
    restart.await.unwrap().unwrap();
    dispose.await.unwrap();

    assert_eq!(fiber_handle.state(), FiberState::Disposed);
    assert_eq!(applies.load(Ordering::SeqCst), 2);
}

#[derive(Debug)]
struct RestartVersion(u32);
impl Service for RestartVersion {
    const NAME: &'static str = "t29-restart-version";
}

struct DriftDuringRestart {
    applies: Arc<AtomicU32>,
    seen: Arc<Mutex<Vec<u32>>>,
    second_apply_started: Arc<tokio::sync::Notify>,
    second_apply_release: Arc<tokio::sync::Notify>,
}
impl Plugin for DriftDuringRestart {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = ApplyBoom;
    fn inject(&self) -> InjectSpec {
        InjectSpec::none().require(RestartVersion::NAME)
    }
    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), ApplyBoom> {
        let attempt = self.applies.fetch_add(1, Ordering::SeqCst) + 1;
        let version = ctx
            .try_service::<RestartVersion>()
            .map_err(|_| ApplyBoom)?
            .0;
        self.seen.lock().push(version);
        if attempt == 2 {
            self.second_apply_started.notify_one();
            self.second_apply_release.notified().await;
        }
        Ok(())
    }
}

#[tokio::test]
async fn restart_keeps_ownership_through_release_time_drift_until_latest_target_is_quiescent() {
    let root = Context::new();
    let first = root.provide(Arc::new(RestartVersion(1))).unwrap();
    let applies = Arc::new(AtomicU32::new(0));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let second_apply_started = Arc::new(tokio::sync::Notify::new());
    let second_apply_release = Arc::new(tokio::sync::Notify::new());
    let fiber_handle = root
        .spawn(prepared(DriftDuringRestart {
            applies: applies.clone(),
            seen: seen.clone(),
            second_apply_started: second_apply_started.clone(),
            second_apply_release: second_apply_release.clone(),
        }))
        .await
        .unwrap();

    let restart = tokio::spawn({
        let fiber_handle = fiber_handle.clone();
        async move { fiber_handle.restart().await }
    });
    second_apply_started.notified().await;
    first.remove().unwrap();
    let _second = root.provide(Arc::new(RestartVersion(2))).unwrap();
    second_apply_release.notify_one();

    restart.await.unwrap().unwrap();
    assert_eq!(fiber_handle.state(), FiberState::Active);
    assert_eq!(applies.load(Ordering::SeqCst), 3);
    assert_eq!(&*seen.lock(), &[1, 1, 2]);
}

struct DisposeBarrier {
    cleanup_started: Arc<tokio::sync::Notify>,
    cleanup_release: Arc<tokio::sync::Notify>,
    cleanups: Arc<AtomicU32>,
}
impl Plugin for DisposeBarrier {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = ApplyBoom;
    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), ApplyBoom> {
        let started = self.cleanup_started.clone();
        let release = self.cleanup_release.clone();
        let cleanups = self.cleanups.clone();
        ctx.effect(move || async move {
            cleanups.fetch_add(1, Ordering::SeqCst);
            started.notify_one();
            release.notified().await;
        })
        .map_err(|_| ApplyBoom)?;
        Ok(())
    }
}

#[test]
fn committed_dispose_survives_origin_runtime_shutdown() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let root = Context::new();
    let cleanup_started = Arc::new(tokio::sync::Notify::new());
    let cleanup_release = Arc::new(tokio::sync::Notify::new());
    let cleanups = Arc::new(AtomicU32::new(0));
    let fiber_handle = runtime
        .block_on(root.spawn(prepared(DisposeBarrier {
            cleanup_started: cleanup_started.clone(),
            cleanup_release: cleanup_release.clone(),
            cleanups: cleanups.clone(),
        })))
        .unwrap();

    let handle = runtime.handle().clone();
    let disposing = fiber_handle.clone();
    let disposer = std::thread::spawn(move || handle.block_on(disposing.dispose()));

    runtime.block_on(cleanup_started.notified());
    assert_eq!(fiber_handle.state(), FiberState::Unloading);
    runtime.shutdown_background();
    cleanup_release.notify_one();

    disposer
        .join()
        .expect("runtime shutdown must not panic a committed disposal")
        .expect("committed disposal still reaches its terminal barrier");
    assert_eq!(cleanups.load(Ordering::SeqCst), 1);
    assert_eq!(fiber_handle.state(), FiberState::Disposed);
}

#[tokio::test]
async fn concurrent_disposals_coalesce_through_cleanup_disposed_and_unlink_after_winner_cancellation()
 {
    let root = Context::new();
    let cleanup_started = Arc::new(tokio::sync::Notify::new());
    let cleanup_release = Arc::new(tokio::sync::Notify::new());
    let cleanups = Arc::new(AtomicU32::new(0));
    let fiber_handle = root
        .spawn(prepared(DisposeBarrier {
            cleanup_started: cleanup_started.clone(),
            cleanup_release: cleanup_release.clone(),
            cleanups: cleanups.clone(),
        }))
        .await
        .unwrap();
    let winner_waiter = tokio::spawn({
        let fiber_handle = fiber_handle.clone();
        async move { fiber_handle.dispose().await }
    });
    cleanup_started.notified().await;
    let coalesced = tokio::spawn({
        let fiber_handle = fiber_handle.clone();
        async move { fiber_handle.dispose().await }
    });
    tokio::task::yield_now().await;
    assert!(
        !coalesced.is_finished(),
        "racing disposer must await the same terminal barrier"
    );
    winner_waiter.abort();
    cleanup_release.notify_one();

    coalesced.await.unwrap().unwrap();
    assert_eq!(cleanups.load(Ordering::SeqCst), 1);
    assert_eq!(fiber_handle.state(), FiberState::Disposed);
    fiber_handle
        .dispose()
        .await
        .expect("completed repeats coalesce successfully");
    tokio::task::yield_now().await;
    assert_eq!(cleanups.load(Ordering::SeqCst), 1);
    assert_eq!(fiber_handle.state(), FiberState::Disposed);
}

struct DropOnlyCleanup {
    ran: Arc<AtomicBool>,
}
impl Plugin for DropOnlyCleanup {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = ApplyBoom;
    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), ApplyBoom> {
        let ran = self.ran.clone();
        ctx.effect_sync(move || ran.store(true, Ordering::SeqCst))
            .map_err(|_| ApplyBoom)?;
        Ok(())
    }
}

#[tokio::test]
async fn dropping_context_handles_never_runs_root_cleanup_and_surviving_runtime_keeps_it_claimable()
{
    let resident_ran = Arc::new(AtomicBool::new(false));
    {
        let root = Context::new();
        let fiber_handle = root
            .spawn(prepared(DropOnlyCleanup {
                ran: resident_ran.clone(),
            }))
            .await
            .unwrap();
        drop(fiber_handle);
        drop(root);
    }
    tokio::task::yield_now().await;
    assert!(
        !resident_ran.load(Ordering::SeqCst),
        "dropping the last Context or FiberHandle clones is storage drop, not implicit resident teardown"
    );

    let ran = Arc::new(AtomicBool::new(false));
    {
        let root = Context::new();
        let ran_cleanup = ran.clone();
        let _registration = root
            .effect_sync(move || {
                ran_cleanup.store(true, Ordering::SeqCst);
            })
            .unwrap();
    }
    tokio::task::yield_now().await;
    assert!(
        !ran.load(Ordering::SeqCst),
        "last Context drop is storage drop, not awaited Runtime/root teardown"
    );

    let root = Context::new();
    let survivor = root.root();
    let ran = Arc::new(AtomicBool::new(false));
    let ran_cleanup = ran.clone();
    let registration = root
        .effect_sync(move || {
            ran_cleanup.store(true, Ordering::SeqCst);
        })
        .unwrap();
    drop(root);
    assert!(
        !ran.load(Ordering::SeqCst),
        "dropping a root handle does not drain root registrations"
    );

    let late_ran = Arc::new(AtomicBool::new(false));
    let late_probe = late_ran.clone();
    let late_registration = survivor
        .effect_sync(move || late_probe.store(true, Ordering::SeqCst))
        .expect("the permanent root generation remains open through surviving handles");
    assert!(!late_ran.load(Ordering::SeqCst));

    assert!(registration.dispose().await.unwrap());
    assert!(late_registration.dispose().await.unwrap());
    assert!(ran.load(Ordering::SeqCst));
    assert!(late_ran.load(Ordering::SeqCst));
    drop(survivor);
}
