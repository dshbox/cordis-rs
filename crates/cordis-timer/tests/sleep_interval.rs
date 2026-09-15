//! Contract tests for the `sleep` and `interval` shapes (port of v1
//! `cordis-timer/tests/timer.rs`'s interval rows plus the sleep seat's
//! born rows; ADR 0012 decision 3 — a shape's tests land with its
//! consumer seat, here the worker daemon).
//!
//! All cases run on tokio's **paused clock** (`start_paused = true`):
//! the deadlines are virtual, so the suite is hermetic against slow CI
//! machines and cancellation can be proven without racing real time.

use std::sync::Arc;
use std::time::Duration;

use cordis_core::{Context, Plugin, PreparedPlugin};
use cordis_timer::{TimerCancelled, TimerExt};
use futures::StreamExt;
use parking_lot::Mutex;
use std::convert::Infallible;
use std::panic::AssertUnwindSafe;
use std::task::{Context as TaskContext, Waker};

/// Plugin that hands its apply-time context to the test — the shapes
/// bind to a fiber, so the tests need a context that *has* one.
struct ContextGrabber {
    captured: Arc<Mutex<Option<Context>>>,
}

impl Plugin for ContextGrabber {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _: &()) -> Result<(), Infallible> {
        *self.captured.lock() = Some(ctx);
        Ok(())
    }
}

/// Spawn one FiberHandle and return `(fiber_handle, its apply-time ctx)`.
async fn scoped_ctx(root: &Context) -> (cordis_core::FiberHandle, Context) {
    let captured: Arc<Mutex<Option<Context>>> = Default::default();
    let fiber_handle = root
        .spawn(PreparedPlugin::from_input(
            ContextGrabber {
                captured: captured.clone(),
            },
            (),
        ))
        .await
        .unwrap();
    let ctx = captured.lock().clone().unwrap();
    (fiber_handle, ctx)
}

/// Scheduler turns for tasks woken by cancellation: on the
/// current-thread runtime a few yields deterministically drain the
/// queue before the assertion reads the result.
async fn settle() {
    for _ in 0..3 {
        tokio::task::yield_now().await;
    }
}

// The `has_timer_entry` introspection probe retired with EF-05: cleanup
// journals expose no labels or listings; timer cancellation is observed
// through each operation's outcome only (ticket TM owns the rewrite).

// ---------------------------------------------------------------------
// sleep
// ---------------------------------------------------------------------

/// Resolved normally after the (virtual) delay → `Ok(())`.
#[tokio::test(start_paused = true)]
async fn sleep_resolves() {
    let root = Context::new();
    let (_fiber_handle, ctx) = scoped_ctx(&root).await;

    let pending = ctx.sleep(Duration::from_millis(10)).unwrap();

    let result = pending.await;
    assert!(matches!(result, Ok(())), "the delay must resolve Ok");
}

/// Fiber disposal cancels the outstanding sleep: the cancellation
/// rides the fiber's watch, not the clock — no virtual advance, the
/// 10 s delay never fires, disposal wins.
#[tokio::test(start_paused = true)]
async fn sleep_cancelled_by_fiber_dispose() {
    let root = Context::new();
    let (fiber_handle, ctx) = scoped_ctx(&root).await;

    let pending = ctx.sleep(Duration::from_millis(10_000)).unwrap();
    let task = tokio::spawn(pending);
    // let the spawned task poll once and park inside the race before
    // disposal, so the cancel wake — not just the standing flag — is
    // what resolves it
    settle().await;

    fiber_handle.dispose().await.unwrap();
    let result = task.await.unwrap();
    assert!(
        matches!(result, Err(TimerCancelled)),
        "fiber disposal must cancel outstanding sleeps"
    );
}

/// A disposed Fiber cannot construct a born-cancelled Sleep: registration
/// refuses synchronously before any Timer value is delivered.
#[tokio::test(start_paused = true)]
async fn sleep_on_dead_fiber_context_refuses_synchronously() {
    let root = Context::new();
    let (fiber_handle, ctx) = scoped_ctx(&root).await;
    fiber_handle.dispose().await.unwrap();

    let result = ctx.sleep(Duration::from_secs(10_000));
    assert!(
        matches!(
            result,
            Err(cordis_timer::TimerRegistrationError::InactiveContext)
        ),
        "a dead fiber refuses Sleep construction synchronously"
    );
}

/// If expiry is already ready but generation cancellation claims the same
/// boundary before the operation commits, cancellation wins.
#[tokio::test(start_paused = true)]
async fn sleep_cancellation_wins_ready_but_uncommitted_expiry() {
    let root = Context::new();
    let (fiber_handle, ctx) = scoped_ctx(&root).await;
    let sleep = ctx.sleep(Duration::from_millis(10)).unwrap();

    tokio::time::advance(Duration::from_millis(10)).await;
    fiber_handle.dispose().await.unwrap();

    assert!(matches!(sleep.await, Err(TimerCancelled)));
}

/// Sleep is a one-shot Future: once its terminal result has been delivered,
/// polling it again is a caller error and follows the documented panic contract.
#[tokio::test(start_paused = true)]
async fn completed_sleep_repoll_panics() {
    let root = Context::new();
    let (_fiber_handle, ctx) = scoped_ctx(&root).await;
    let mut sleep = Box::pin(ctx.sleep(Duration::ZERO).unwrap());

    assert!(sleep.as_mut().await.is_ok());

    let waker = Waker::noop();
    let mut task_cx = TaskContext::from_waker(waker);
    let repoll = std::panic::catch_unwind(AssertUnwindSafe(|| {
        std::future::Future::poll(sleep.as_mut(), &mut task_cx)
    }));
    assert!(repoll.is_err(), "completed Sleep must reject a second poll");
}

/// Natural completion disarms its exact cleanup occurrence. Later generation
/// disposal cannot retroactively turn the completed operation into cancellation.
#[tokio::test(start_paused = true)]
async fn completed_sleep_is_not_reacted_to_by_later_generation_disposal() {
    let root = Context::new();
    let (fiber_handle, ctx) = scoped_ctx(&root).await;
    let sleep = ctx.sleep(Duration::from_millis(10)).unwrap();

    tokio::time::advance(Duration::from_millis(10)).await;
    assert!(matches!(sleep.await, Ok(())));
    fiber_handle.dispose().await.unwrap();
}

// ---------------------------------------------------------------------
// interval
// ---------------------------------------------------------------------

fn assert_interval_type(_: cordis_timer::Interval) {}

/// Construction fixes the phase immediately: the first tick is exactly
/// anchor + period, even when the stream is not polled until later.
#[tokio::test(start_paused = true)]
async fn interval_first_tick_is_anchored_at_construction() {
    let root = Context::new();
    let (_fiber_handle, ctx) = scoped_ctx(&root).await;
    let anchor = tokio::time::Instant::now();
    let interval = ctx.interval(Duration::from_millis(10)).unwrap();
    assert_interval_type(interval);

    let mut interval = Box::pin(ctx.interval(Duration::from_millis(10)).unwrap());
    tokio::time::advance(Duration::from_millis(9)).await;
    assert!(futures::poll!(interval.as_mut().next()).is_pending());
    tokio::time::advance(Duration::from_millis(1)).await;
    assert!(matches!(interval.next().await, Some(Ok(()))));
    assert_eq!(
        tokio::time::Instant::now(),
        anchor + Duration::from_millis(10)
    );
}

/// On-time observation yields one successful item per phase deadline.
#[tokio::test(start_paused = true)]
async fn interval_on_time_ticks_are_ok() {
    let root = Context::new();
    let (_fiber_handle, ctx) = scoped_ctx(&root).await;
    let mut interval = Box::pin(ctx.interval(Duration::from_millis(5)).unwrap());

    for _ in 0..3 {
        tokio::time::advance(Duration::from_millis(5)).await;
        assert!(matches!(interval.next().await, Some(Ok(()))));
    }
}

/// A late observation coalesces all missed ticks to one overdue item, then
/// resumes at the first original-phase deadline strictly after observation.
#[tokio::test(start_paused = true)]
async fn interval_late_poll_coalesces_without_burst_or_phase_shift() {
    let root = Context::new();
    let (_fiber_handle, ctx) = scoped_ctx(&root).await;
    let anchor = tokio::time::Instant::now();
    let mut interval = Box::pin(ctx.interval(Duration::from_millis(10)).unwrap());

    tokio::time::advance(Duration::from_millis(35)).await;
    assert!(matches!(interval.next().await, Some(Ok(()))));
    assert_eq!(
        tokio::time::Instant::now(),
        anchor + Duration::from_millis(35)
    );
    assert!(
        futures::poll!(interval.as_mut().next()).is_pending(),
        "missed ticks must not burst after the one coalesced overdue tick"
    );

    tokio::time::advance(Duration::from_millis(4)).await;
    assert!(futures::poll!(interval.as_mut().next()).is_pending());
    tokio::time::advance(Duration::from_millis(1)).await;
    assert!(matches!(interval.next().await, Some(Ok(()))));
    assert_eq!(
        tokio::time::Instant::now(),
        anchor + Duration::from_millis(40),
        "cadence must remain on the construction-time phase"
    );
}

/// Generation cancellation is explicit: exactly one error item, then the
/// stream is permanently terminated even if time advances further.
#[tokio::test(start_paused = true)]
async fn interval_cancellation_yields_one_error_then_ends() {
    let root = Context::new();
    let (fiber_handle, ctx) = scoped_ctx(&root).await;
    let mut interval = Box::pin(ctx.interval(Duration::from_millis(10)).unwrap());

    fiber_handle.dispose().await.unwrap();
    assert!(matches!(interval.next().await, Some(Err(TimerCancelled))));
    assert!(interval.next().await.is_none());
    tokio::time::advance(Duration::from_millis(100)).await;
    assert!(interval.next().await.is_none());
}

/// Cancellation already committed at an exact tick boundary wins the ready
/// but unobserved tick deterministically.
#[tokio::test(start_paused = true)]
async fn interval_cancellation_wins_uncommitted_boundary_tick() {
    let root = Context::new();
    let (fiber_handle, ctx) = scoped_ctx(&root).await;
    let mut interval = Box::pin(ctx.interval(Duration::from_millis(10)).unwrap());

    tokio::time::advance(Duration::from_millis(10)).await;
    fiber_handle.dispose().await.unwrap();
    assert!(matches!(interval.next().await, Some(Err(TimerCancelled))));
    assert!(interval.next().await.is_none());
}

/// Dropping Interval is abandonment only: later generation disposal has no
/// stream value to emit and must complete normally.
#[tokio::test(start_paused = true)]
async fn dropping_interval_emits_nothing() {
    let root = Context::new();
    let (fiber_handle, ctx) = scoped_ctx(&root).await;
    let interval = ctx.interval(Duration::from_millis(10)).unwrap();
    drop(interval);
    fiber_handle.dispose().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn interval_on_dead_fiber_context_refuses_synchronously() {
    let root = Context::new();
    let (fiber_handle, ctx) = scoped_ctx(&root).await;
    fiber_handle.dispose().await.unwrap();

    let result = ctx.interval(Duration::from_secs(10_000));
    assert!(matches!(
        result,
        Err(cordis_timer::TimerRegistrationError::InactiveContext)
    ));
}
