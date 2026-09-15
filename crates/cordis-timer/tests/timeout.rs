//! Issue 46 contract evidence for work-owning Timeout.

use std::cell::Cell;
use std::convert::Infallible;
use std::future::{Future, pending, ready};
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll, Waker};
use std::time::Duration;

use cordis_core::{Context, Plugin, PreparedPlugin};
use cordis_timer::{TimeoutOutcome, TimerCancelled, TimerExt, TimerRegistrationError};
use parking_lot::Mutex;

struct Capture(Arc<Mutex<Option<Context>>>);

impl Plugin for Capture {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _: &()) -> Result<(), Infallible> {
        *self.0.lock() = Some(ctx);
        Ok(())
    }
}

async fn scoped_ctx(root: &Context) -> (cordis_core::FiberHandle, Context) {
    let captured = Arc::new(Mutex::new(None));
    let fiber_handle = root
        .spawn(PreparedPlugin::from_input(Capture(captured.clone()), ()))
        .await
        .unwrap();
    let ctx = captured.lock().clone().unwrap();
    (fiber_handle, ctx)
}

struct PollCount<'a> {
    polls: &'a Cell<usize>,
    ready: bool,
}
impl Future for PollCount<'_> {
    type Output = &'static str;
    fn poll(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        self.polls.set(self.polls.get() + 1);
        if self.ready {
            Poll::Ready("done")
        } else {
            Poll::Pending
        }
    }
}

#[tokio::test(start_paused = true)]
async fn construction_owns_work_without_polling_it() {
    let ctx = Context::new();
    let polls = Cell::new(0);
    let timeout = ctx
        .timeout(
            Duration::from_secs(1),
            PollCount {
                polls: &polls,
                ready: true,
            },
        )
        .unwrap();
    assert_eq!(polls.get(), 0, "construction must leave work lazy");
    drop(timeout);
    assert_eq!(polls.get(), 0, "abandonment must not poll work");
}

#[test]
fn timeout_outcome_is_debug_when_output_is_debug() {
    assert_eq!(
        format!("{:?}", TimeoutOutcome::Completed("value")),
        "Completed(\"value\")"
    );
    assert_eq!(format!("{:?}", TimeoutOutcome::<()>::Elapsed), "Elapsed");
}

#[tokio::test(start_paused = true)]
async fn work_ready_before_deadline_returns_completed_output() {
    let ctx = Context::new();
    let result = ctx
        .timeout(Duration::from_secs(10), ready("value"))
        .unwrap()
        .await;
    assert!(matches!(result, Ok(TimeoutOutcome::Completed("value"))));
}

/// Timeout is also one-shot; retaining it after completion does not make a
/// second poll valid, even when the work output would happen to be Copy.
#[tokio::test(start_paused = true)]
async fn completed_timeout_repoll_panics() {
    let ctx = Context::new();
    let mut timeout = Box::pin(
        ctx.timeout(Duration::from_secs(10), ready("value"))
            .unwrap(),
    );

    assert!(matches!(
        timeout.as_mut().await,
        Ok(TimeoutOutcome::Completed("value"))
    ));

    let waker = Waker::noop();
    let mut task_cx = TaskContext::from_waker(waker);
    let repoll = std::panic::catch_unwind(AssertUnwindSafe(|| {
        Future::poll(timeout.as_mut(), &mut task_cx)
    }));
    assert!(
        repoll.is_err(),
        "completed Timeout must reject a second poll"
    );
}

#[tokio::test(start_paused = true)]
async fn deadline_already_elapsed_drops_work_without_polling_it() {
    let ctx = Context::new();
    let polls = Cell::new(0);
    let timeout = ctx
        .timeout(
            Duration::from_secs(5),
            PollCount {
                polls: &polls,
                ready: false,
            },
        )
        .unwrap();
    tokio::time::advance(Duration::from_secs(5)).await;
    let result = timeout.await;
    assert!(matches!(result, Ok(TimeoutOutcome::Elapsed)));
    assert_eq!(
        polls.get(),
        0,
        "an elapsed pinned deadline wins before work is polled"
    );
}

#[tokio::test(start_paused = true)]
async fn generation_cancellation_is_distinct_from_elapsed() {
    let root = Context::new();
    let (fiber_handle, ctx) = scoped_ctx(&root).await;
    let timeout = ctx
        .timeout(Duration::from_secs(60), pending::<()>())
        .unwrap();
    tokio::pin!(timeout);
    assert!(futures::poll!(timeout.as_mut()).is_pending());
    fiber_handle.dispose().await.unwrap();
    assert!(matches!(timeout.await, Err(TimerCancelled)));
}

#[tokio::test(start_paused = true)]
async fn standing_generation_cancellation_wins_before_work_poll() {
    let root = Context::new();
    let (fiber_handle, ctx) = scoped_ctx(&root).await;
    let polls = Cell::new(0);
    let timeout = ctx
        .timeout(
            Duration::from_secs(60),
            PollCount {
                polls: &polls,
                ready: true,
            },
        )
        .unwrap();
    fiber_handle.dispose().await.unwrap();
    assert!(matches!(timeout.await, Err(TimerCancelled)));
    assert_eq!(polls.get(), 0);
}

#[tokio::test(start_paused = true)]
async fn timeout_accepts_borrowing_non_send_work_and_non_send_output() {
    let ctx = Context::new();
    let local = Rc::new(String::from("local"));
    let borrowed = local.as_str();
    let result = ctx
        .timeout(Duration::from_secs(1), async move { Rc::new(borrowed) })
        .unwrap()
        .await;
    match result {
        Ok(TimeoutOutcome::Completed(output)) => assert_eq!(*output, "local"),
        _ => panic!("unexpected timeout result"),
    }
}

struct DropMark<'a>(&'a Cell<usize>);
impl Drop for DropMark<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

#[tokio::test(start_paused = true)]
async fn generation_cleanup_never_owns_or_drops_timeout_work() {
    let root = Context::new();
    let (fiber_handle, ctx) = scoped_ctx(&root).await;
    let drops = Cell::new(0);
    let mark = DropMark(&drops);
    let timeout = ctx
        .timeout(Duration::from_secs(60), async move {
            let _mark = mark;
            pending::<()>().await;
        })
        .unwrap();

    fiber_handle.dispose().await.unwrap();
    assert_eq!(
        drops.get(),
        0,
        "generation cleanup must not drop caller work"
    );
    drop(timeout);
    assert_eq!(
        drops.get(),
        1,
        "caller-side operation Drop owns caller work"
    );
}

#[test]
fn timeout_refuses_off_runtime_synchronously_without_polling_work() {
    let ctx = Context::new();
    let polls = Cell::new(0);
    let result = ctx.timeout(
        Duration::from_millis(1),
        PollCount {
            polls: &polls,
            ready: true,
        },
    );
    assert!(matches!(
        result,
        Err(TimerRegistrationError::TimerUnavailable)
    ));
    assert_eq!(polls.get(), 0);
}

#[tokio::test(start_paused = true)]
async fn cancellation_wins_ready_but_uncommitted_timeout_deadline() {
    let root = Context::new();
    let (fiber_handle, ctx) = scoped_ctx(&root).await;
    let polls = Cell::new(0);
    let timeout = ctx
        .timeout(
            Duration::from_secs(5),
            PollCount {
                polls: &polls,
                ready: true,
            },
        )
        .unwrap();

    tokio::time::advance(Duration::from_secs(5)).await;
    fiber_handle.dispose().await.unwrap();

    assert!(matches!(timeout.await, Err(TimerCancelled)));
    assert_eq!(
        polls.get(),
        0,
        "cancellation/deadline boundary must not poll work"
    );
}

struct DropThread<'a> {
    dropped_on: &'a Cell<Option<std::thread::ThreadId>>,
}

impl Drop for DropThread<'_> {
    fn drop(&mut self) {
        self.dropped_on.set(Some(std::thread::current().id()));
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn cancelled_timeout_work_is_dropped_by_the_caller_poll() {
    let root = Context::new();
    let (fiber_handle, ctx) = scoped_ctx(&root).await;
    let dropped_on = Cell::new(None);
    let caller = std::thread::current().id();
    let marker = DropThread {
        dropped_on: &dropped_on,
    };
    let timeout = ctx
        .timeout(Duration::from_secs(60), async move {
            let _marker = marker;
            pending::<()>().await;
        })
        .unwrap();
    tokio::pin!(timeout);

    assert!(futures::poll!(timeout.as_mut()).is_pending());
    fiber_handle.dispose().await.unwrap();
    assert_eq!(
        dropped_on.get(),
        None,
        "generation cleanup must not own work destruction"
    );
    assert!(matches!(timeout.as_mut().await, Err(TimerCancelled)));
    assert_eq!(
        dropped_on.get(),
        Some(caller),
        "terminal work destruction stays in the caller poll frame"
    );
}

struct ReentrantPoll {
    ctx: Context,
    polls: Arc<std::sync::atomic::AtomicUsize>,
}

impl Future for ReentrantPoll {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        self.ctx
            .effect_sync(|| {})
            .expect("Timer work may register cleanup while it is polled");
        self.polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Poll::Ready(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timeout_work_poll_can_reenter_generation_bookkeeping() {
    let root = Context::new();
    let polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let operation = root
        .timeout(
            Duration::from_secs(60),
            ReentrantPoll {
                ctx: root.clone(),
                polls: polls.clone(),
            },
        )
        .unwrap();
    let task = tokio::spawn(operation);
    let outcome = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("Timer work polling deadlocked generation bookkeeping")
        .unwrap()
        .unwrap();
    assert!(matches!(outcome, TimeoutOutcome::Completed(())));
    assert_eq!(polls.load(std::sync::atomic::Ordering::SeqCst), 1);
}
