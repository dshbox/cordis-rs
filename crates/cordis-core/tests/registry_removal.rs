//! Issue 30 evidence for exact typed PluginGroup removal.

use cordis_core::lifecycle::LifecycleOperation;
use cordis_core::{Context, FiberState, Plugin, PreparedPlugin};
use parking_lot::Mutex;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

fn prepared<P>(plugin: P) -> PreparedPlugin
where
    P: Plugin<Config = (), Input = (), PrepareError = Infallible>,
{
    PreparedPlugin::from_input(plugin, ())
}

struct Plain;
impl Plugin for Plain {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, _ctx: Context, _prepared: &()) -> Result<(), Infallible> {
        Ok(())
    }
}

#[tokio::test]
async fn typed_removal_freezes_the_detached_allocation_and_repeated_absence_succeeds() {
    let ctx = Context::new();
    let a = ctx.spawn(prepared(Plain)).await.unwrap();
    let b = ctx.spawn(prepared(Plain)).await.unwrap();

    ctx.remove_plugins::<Plain>().await.unwrap();
    assert_eq!(a.state(), FiberState::Disposed);
    assert_eq!(b.state(), FiberState::Disposed);

    ctx.remove_plugins::<Plain>().await.unwrap();

    let fresh = ctx.spawn(prepared(Plain)).await.unwrap();
    assert_eq!(fresh.state(), FiberState::Active);
    ctx.remove_plugins::<Plain>().await.unwrap();
    assert_eq!(fresh.state(), FiberState::Disposed);
}

struct BlockingCleanup {
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    finished: Arc<AtomicBool>,
}
impl Plugin for BlockingCleanup {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), Infallible> {
        let started = self.started.clone();
        let release = self.release.clone();
        let finished = self.finished.clone();
        ctx.effect(move || async move {
            started.notify_one();
            release.notified().await;
            finished.store(true, Ordering::SeqCst);
        })
        .unwrap();
        Ok(())
    }
}

#[tokio::test]
async fn detach_commits_removal_and_caller_cancellation_cannot_stop_the_frozen_drain() {
    let ctx = Context::new();
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let finished = Arc::new(AtomicBool::new(false));
    let old = ctx
        .spawn(prepared(BlockingCleanup {
            started: started.clone(),
            release: release.clone(),
            finished: finished.clone(),
        }))
        .await
        .unwrap();

    let remover = tokio::spawn({
        let ctx = ctx.clone();
        async move { ctx.remove_plugins::<BlockingCleanup>().await }
    });
    started.notified().await;

    // Reaching ordinary cleanup proves the old allocation has already detached.
    // A same-type attach from here must therefore use a fresh allocation.
    let fresh_started = Arc::new(tokio::sync::Notify::new());
    let fresh_release = Arc::new(tokio::sync::Notify::new());
    let fresh = ctx
        .spawn(prepared(BlockingCleanup {
            started: fresh_started,
            release: fresh_release.clone(),
            finished: Arc::new(AtomicBool::new(false)),
        }))
        .await
        .unwrap();
    remover.abort();
    release.notify_one();

    old.wait_state(FiberState::Disposed, Duration::from_secs(2))
        .await
        .expect("framework-owned removal reaches terminal publication");
    assert!(finished.load(Ordering::SeqCst));
    assert_eq!(
        fresh.state(),
        FiberState::Active,
        "fresh allocation survives old removal"
    );

    fresh_release.notify_one();
    ctx.remove_plugins::<BlockingCleanup>().await.unwrap();
    assert_eq!(fresh.state(), FiberState::Disposed);
}

#[test]
fn committed_removal_survives_runtime_shutdown() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let ctx = Context::new();
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let finished = Arc::new(AtomicBool::new(false));

    runtime.block_on(async {
        ctx.spawn(prepared(BlockingCleanup {
            started: started.clone(),
            release: release.clone(),
            finished: finished.clone(),
        }))
        .await
        .unwrap();
    });

    let handle = runtime.handle().clone();
    let remover_ctx = ctx.clone();
    let remover = std::thread::spawn(move || {
        handle.block_on(remover_ctx.remove_plugins::<BlockingCleanup>())
    });

    runtime.block_on(started.notified());
    runtime.shutdown_background();
    release.notify_one();

    remover
        .join()
        .expect("runtime shutdown must not panic a committed removal")
        .unwrap();
    assert!(
        finished.load(Ordering::SeqCst),
        "framework-owned cleanup must finish after executor shutdown"
    );
}

#[derive(Clone, Copy)]
enum CleanupMode {
    Panics,
    Fails,
    Completes,
}

#[derive(Debug, thiserror::Error)]
#[error("issue-30 cleanup failure")]
struct CleanupFailure;

struct CleanupContainment {
    mode: CleanupMode,
    completed: Arc<AtomicUsize>,
}
impl Plugin for CleanupContainment {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), Infallible> {
        match self.mode {
            CleanupMode::Panics => {
                ctx.effect_sync(|| -> () { panic!("issue-30 cleanup panic") })
                    .unwrap();
            }
            CleanupMode::Fails => {
                ctx.effect_sync(|| -> Result<(), CleanupFailure> { Err(CleanupFailure) })
                    .unwrap();
            }
            CleanupMode::Completes => {
                let completed = self.completed.clone();
                ctx.effect_sync(move || {
                    completed.fetch_add(1, Ordering::SeqCst);
                })
                .unwrap();
            }
        }
        Ok(())
    }
}

#[tokio::test]
async fn cleanup_failure_and_panic_do_not_stop_other_frozen_members() {
    let ctx = Context::new();
    let completed = Arc::new(AtomicUsize::new(0));
    let panicky = ctx
        .spawn(prepared(CleanupContainment {
            mode: CleanupMode::Panics,
            completed: completed.clone(),
        }))
        .await
        .unwrap();
    let failing = ctx
        .spawn(prepared(CleanupContainment {
            mode: CleanupMode::Fails,
            completed: completed.clone(),
        }))
        .await
        .unwrap();
    let healthy = ctx
        .spawn(prepared(CleanupContainment {
            mode: CleanupMode::Completes,
            completed: completed.clone(),
        }))
        .await
        .unwrap();

    ctx.remove_plugins::<CleanupContainment>().await.unwrap();

    assert_eq!(panicky.state(), FiberState::Disposed);
    assert_eq!(failing.state(), FiberState::Disposed);
    assert_eq!(healthy.state(), FiberState::Disposed);
    assert_eq!(completed.load(Ordering::SeqCst), 1);
}

struct RecursingRemoval {
    seen: Arc<Mutex<Option<(LifecycleOperation, cordis_core::FiberId)>>>,
}
impl Plugin for RecursingRemoval {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), Infallible> {
        let recursion = ctx
            .remove_plugins::<Self>()
            .await
            .expect_err("self-removal must refuse");
        *self.seen.lock() = Some((recursion.operation(), recursion.fiber_id().clone()));
        Ok(())
    }
}

#[tokio::test]
async fn self_wait_recursion_is_refused_before_typed_group_detach() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(None));
    let fiber_handle = ctx
        .spawn(prepared(RecursingRemoval { seen: seen.clone() }))
        .await
        .unwrap();

    let recorded = seen.lock().clone().expect("apply observed refusal");
    assert_eq!(recorded.0, LifecycleOperation::RemovePlugins);
    assert_eq!(recorded.1, fiber_handle.id());
    assert_eq!(
        fiber_handle.state(),
        FiberState::Active,
        "refusal did not detach the group"
    );

    ctx.remove_plugins::<RecursingRemoval>().await.unwrap();
    assert_eq!(fiber_handle.state(), FiberState::Disposed);
}

struct RemovesPlainFromAnotherFiber {
    completed: Arc<AtomicBool>,
}
impl Plugin for RemovesPlainFromAnotherFiber {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), Infallible> {
        ctx.remove_plugins::<Plain>()
            .await
            .expect("unrelated Fiber may remove another group");
        self.completed.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn unrelated_settle_context_may_remove_another_typed_group() {
    let ctx = Context::new();
    let target = ctx.spawn(prepared(Plain)).await.unwrap();
    let completed = Arc::new(AtomicBool::new(false));

    let remover = ctx
        .spawn(prepared(RemovesPlainFromAnotherFiber {
            completed: completed.clone(),
        }))
        .await
        .unwrap();

    assert!(completed.load(Ordering::SeqCst));
    assert_eq!(target.state(), FiberState::Disposed);
    assert_eq!(remover.state(), FiberState::Active);
}
