//! Issue 54 Timer evidence: the leaf operation stays owned by the Context
//! generation that registered its core cleanup, regardless of where it is used.

use cordis_core::service::ServicePublication;
use cordis_core::{Context, FiberState, InjectSpec, Plugin, PreparedPlugin, Service};
use cordis_timer::{Sleep, TimerCancelled, TimerExt, TimerRegistrationError};
use parking_lot::Mutex;
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;

#[derive(Debug)]
struct GateDependency;
impl Service for GateDependency {
    const NAME: &'static str = "issue54/timer-gate-dependency";
}

struct PollProbe(Arc<AtomicUsize>);
impl Future for PollProbe {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Poll::Ready(())
    }
}

#[derive(Clone)]
struct CapturePlugin {
    slot: Arc<Mutex<Option<Context>>>,
}
impl Plugin for CapturePlugin {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _: &()) -> Result<(), Infallible> {
        *self.slot.lock() = Some(ctx);
        Ok(())
    }
}

struct GatePlugin {
    slot: Arc<Mutex<Option<Context>>>,
    fail: Arc<AtomicBool>,
}
impl Plugin for GatePlugin {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = std::io::Error;

    fn inject(&self) -> InjectSpec {
        InjectSpec::none().require(GateDependency::NAME)
    }

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _: &()) -> Result<(), std::io::Error> {
        *self.slot.lock() = Some(ctx);
        if self.fail.load(Ordering::SeqCst) {
            Err(std::io::Error::other("issue54 forced timer owner failure"))
        } else {
            Ok(())
        }
    }
}

struct LoadingTimer {
    delivered: Arc<Mutex<Option<Sleep>>>,
}
impl Plugin for LoadingTimer {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _: &()) -> Result<(), Infallible> {
        *self.delivered.lock() = Some(
            ctx.sleep(Duration::from_secs(3600))
                .expect("Loading admits Timer cleanup registration"),
        );
        Ok(())
    }
}

struct TimerChild {
    delivered: Arc<Mutex<Option<Sleep>>>,
}
impl Plugin for TimerChild {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _: &()) -> Result<(), Infallible> {
        *self.delivered.lock() = Some(ctx.sleep(Duration::from_secs(3600)).unwrap());
        Ok(())
    }
}

fn prepared<P>(plugin: P) -> PreparedPlugin
where
    P: Plugin<Config = (), Input = (), PrepareError = Infallible>,
{
    PreparedPlugin::from_input(plugin, ())
}

async fn scoped_context(root: &Context) -> (cordis_core::FiberHandle, Context) {
    let slot = Arc::new(Mutex::new(None));
    let fiber_handle = root
        .spawn(prepared(CapturePlugin { slot: slot.clone() }))
        .await
        .unwrap();
    let ctx = slot.lock().clone().unwrap();
    (fiber_handle, ctx)
}

fn assert_timer_refused(ctx: &Context) {
    assert!(matches!(
        ctx.sleep(Duration::from_secs(1)),
        Err(TimerRegistrationError::InactiveContext)
    ));

    let work_polls = Arc::new(AtomicUsize::new(0));
    match ctx.timeout(Duration::from_secs(1), PollProbe(work_polls.clone())) {
        Err(TimerRegistrationError::InactiveContext) => {}
        Err(other) => panic!("unexpected Timeout refusal: {other}"),
        Ok(_) => panic!("inactive generation delivered a Timeout"),
    }
    assert_eq!(work_polls.load(Ordering::SeqCst), 0);

    assert!(matches!(
        ctx.interval(Duration::from_secs(1)),
        Err(TimerRegistrationError::InactiveContext)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loading_active_and_permanent_root_admit_timer_operations() {
    let root = Context::new();
    let loading_sleep = Arc::new(Mutex::new(None));
    let loading_owner = root
        .spawn(prepared(LoadingTimer {
            delivered: loading_sleep.clone(),
        }))
        .await
        .unwrap();
    assert_eq!(loading_owner.state(), FiberState::Active);
    let loading_sleep = loading_sleep.lock().take().unwrap();
    loading_owner.dispose().await.unwrap();
    assert!(matches!(loading_sleep.await, Err(TimerCancelled)));

    let (active_owner, active_ctx) = scoped_context(&root).await;
    let active_sleep = active_ctx.sleep(Duration::from_secs(3600)).unwrap();
    active_owner.dispose().await.unwrap();
    assert!(matches!(active_sleep.await, Err(TimerCancelled)));

    // The root's generation is permanent for the Runtime lifetime: ordinary
    // Fiber close does not close it, and it still admits fresh Timer cleanup.
    assert!(root.sleep(Duration::ZERO).unwrap().await.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pending_failed_closing_and_disposed_refuse_all_timer_constructors() {
    let pending_root = Context::new();
    let dependency: ServicePublication<GateDependency> =
        pending_root.provide(Arc::new(GateDependency)).unwrap();
    let pending_slot = Arc::new(Mutex::new(None));
    let pending = pending_root
        .spawn(prepared(GatePlugin {
            slot: pending_slot.clone(),
            fail: Arc::new(AtomicBool::new(false)),
        }))
        .await
        .unwrap();
    let pending_ctx = pending_slot.lock().clone().unwrap();
    dependency.remove().unwrap();
    assert_eq!(pending.ready().await.unwrap(), FiberState::Pending);
    assert_timer_refused(&pending_ctx);
    pending.dispose().await.unwrap();

    let failed_root = Context::new();
    let _dependency = failed_root.provide(Arc::new(GateDependency)).unwrap();
    let failed_slot = Arc::new(Mutex::new(None));
    let fail = Arc::new(AtomicBool::new(false));
    let failed = failed_root
        .spawn(prepared(GatePlugin {
            slot: failed_slot.clone(),
            fail: fail.clone(),
        }))
        .await
        .unwrap();
    fail.store(true, Ordering::SeqCst);
    assert!(failed.restart().await.is_err());
    assert_eq!(failed.state(), FiberState::Failed);
    assert_timer_refused(&failed_slot.lock().clone().unwrap());
    failed.dispose().await.unwrap();

    let closing_root = Context::new();
    let (closing_owner, closing_ctx) = scoped_context(&closing_root).await;
    let cleanup_started = Arc::new(tokio::sync::Notify::new());
    let cleanup_release = Arc::new(tokio::sync::Notify::new());
    closing_ctx
        .effect({
            let cleanup_started = cleanup_started.clone();
            let cleanup_release = cleanup_release.clone();
            move || async move {
                cleanup_started.notify_one();
                cleanup_release.notified().await;
            }
        })
        .unwrap();
    let disposer = tokio::spawn({
        let closing_owner = closing_owner.clone();
        async move { closing_owner.dispose().await }
    });
    cleanup_started.notified().await;
    assert_timer_refused(&closing_ctx);
    cleanup_release.notify_one();
    disposer.await.unwrap().unwrap();
    assert_eq!(closing_owner.state(), FiberState::Disposed);
    assert_timer_refused(&closing_ctx);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn timer_publish_versus_close_is_refusal_or_one_delivered_cancelled_operation() {
    let root = Context::new();
    let (owner, ctx) = scoped_context(&root).await;
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let registration = tokio::spawn({
        let barrier = barrier.clone();
        async move {
            barrier.wait().await;
            ctx.sleep(Duration::from_secs(3600))
        }
    });
    let close = tokio::spawn({
        let barrier = barrier.clone();
        let owner = owner.clone();
        async move {
            barrier.wait().await;
            owner.dispose().await.unwrap();
        }
    });
    barrier.wait().await;
    let outcome = registration.await.unwrap();
    close.await.unwrap();

    match outcome {
        Ok(sleep) => assert!(matches!(sleep.await, Err(TimerCancelled))),
        Err(TimerRegistrationError::InactiveContext) => {}
        Err(other) => panic!("unexpected Timer race refusal: {other}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cross_fiber_timer_use_keeps_registering_owner_restart_replaces_and_spawn_is_not_parenthood()
 {
    let root = Context::new();
    let (owner, owner_ctx) = scoped_context(&root).await;
    let (user, user_ctx) = scoped_context(&root).await;
    let (send_sleep, receive_sleep) = tokio::sync::oneshot::channel();

    // This constructor executes in `user`'s cooperative task, but the Context
    // passed to Timer is `owner_ctx`; caller attribution must not transfer the
    // generation cleanup owner.
    user_ctx
        .run({
            let owner_ctx = owner_ctx.clone();
            async move {
                let sleep = owner_ctx.sleep(Duration::from_secs(3600)).unwrap();
                let _ = send_sleep.send(sleep);
            }
        })
        .unwrap();
    let sleep = receive_sleep.await.unwrap();
    let mut sleep = Box::pin(sleep);

    user.dispose().await.unwrap();
    assert!(matches!(futures::poll!(sleep.as_mut()), Poll::Pending));

    owner.restart().await.unwrap();
    assert!(matches!(sleep.await, Err(TimerCancelled)));

    let replacement = owner_ctx.sleep(Duration::from_secs(3600)).unwrap();

    // A Timer created by a child Fiber spawned from the owner Context belongs
    // to that child, not to spawn origin. Parent disposal therefore leaves it
    // pending; child disposal owns the cancellation.
    let child_sleep = Arc::new(Mutex::new(None));
    let child = owner_ctx
        .spawn(prepared(TimerChild {
            delivered: child_sleep.clone(),
        }))
        .await
        .unwrap();
    let mut child_sleep = Box::pin(child_sleep.lock().take().unwrap());

    owner.dispose().await.unwrap();
    assert!(matches!(replacement.await, Err(TimerCancelled)));
    assert!(matches!(
        futures::poll!(child_sleep.as_mut()),
        Poll::Pending
    ));

    child.dispose().await.unwrap();
    assert!(matches!(child_sleep.await, Err(TimerCancelled)));
}
