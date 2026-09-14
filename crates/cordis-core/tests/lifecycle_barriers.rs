//! Issue 29 evidence for restart, terminal disposal, and permanent-root barriers.

mod common;

use cordis_core::lifecycle::{LifecycleOperation, PluginFailureKind, RestartError};
use cordis_core::{Context, FiberState, InjectSpec, Plugin, PreparedPlugin, Service};
use parking_lot::Mutex;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

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
