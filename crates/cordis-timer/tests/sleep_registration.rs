//! Issue 45 contract evidence: complete Sleep registration is synchronous.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use cordis_core::{Context, Plugin, PreparedPlugin};
use cordis_timer::{TimerCancelled, TimerExt, TimerRegistrationError};
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

#[tokio::test(start_paused = true)]
async fn zero_sleep_is_valid_and_deadline_is_pinned_at_construction() {
    let ctx = Context::new();
    assert!(ctx.sleep(Duration::ZERO).unwrap().await.is_ok());

    let start = tokio::time::Instant::now();
    let sleep = ctx.sleep(Duration::from_millis(10)).unwrap();
    tokio::time::advance(Duration::from_millis(10)).await;
    assert!(sleep.await.is_ok());
    assert_eq!(
        tokio::time::Instant::now(),
        start + Duration::from_millis(10)
    );
}

#[test]
fn sleep_off_runtime_refuses_synchronously() {
    let ctx = Context::new();
    assert!(matches!(
        ctx.sleep(Duration::from_millis(1)),
        Err(TimerRegistrationError::TimerUnavailable)
    ));
}

#[test]
fn inactive_context_precedes_timer_environment_validation() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (fiber_handle, ctx) = rt.block_on(async {
        let root = Context::new();
        let pair = scoped_ctx(&root).await;
        pair.0.dispose().await.unwrap();
        pair
    });
    drop(fiber_handle);
    assert!(matches!(
        ctx.sleep(Duration::MAX),
        Err(TimerRegistrationError::InactiveContext)
    ));
}

#[test]
fn runtime_without_time_driver_refuses_instead_of_panicking() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let ctx = Context::new();
    let _guard = rt.enter();
    assert!(matches!(
        ctx.sleep(Duration::from_millis(1)),
        Err(TimerRegistrationError::TimerUnavailable)
    ));
}

#[tokio::test(start_paused = true)]
async fn out_of_range_deadline_is_a_registration_error() {
    let ctx = Context::new();
    assert!(matches!(
        ctx.sleep(Duration::MAX),
        Err(TimerRegistrationError::DeadlineOutOfRange)
    ));
}

#[tokio::test(start_paused = true)]
async fn generation_cleanup_cancels_a_constructed_sleep_once() {
    let root = Context::new();
    let (fiber_handle, ctx) = scoped_ctx(&root).await;
    let sleep = ctx.sleep(Duration::from_secs(60)).unwrap();
    let task = tokio::spawn(sleep);
    tokio::task::yield_now().await;
    fiber_handle.dispose().await.unwrap();
    assert!(matches!(task.await.unwrap(), Err(TimerCancelled)));
}

#[tokio::test(start_paused = true)]
async fn constructed_sleep_can_be_polled_from_another_task() {
    let ctx = Context::new();
    let sleep = ctx.sleep(Duration::from_millis(5)).unwrap();
    let task = tokio::spawn(sleep);
    tokio::time::advance(Duration::from_millis(5)).await;
    assert!(matches!(task.await.unwrap(), Ok(())));
}

#[test]
fn all_public_timer_constructors_refuse_off_runtime() {
    let ctx = Context::new();
    assert!(matches!(
        ctx.timeout(Duration::from_millis(1), std::future::pending::<()>()),
        Err(TimerRegistrationError::TimerUnavailable)
    ));
    assert!(matches!(
        ctx.interval(Duration::from_millis(1)),
        Err(TimerRegistrationError::TimerUnavailable)
    ));
}

#[test]
fn zero_interval_precedes_context_and_timer_validation() {
    let ctx = Context::new();
    assert!(matches!(
        ctx.interval(Duration::ZERO),
        Err(TimerRegistrationError::ZeroPeriod)
    ));
}

#[tokio::test(start_paused = true)]
async fn standing_generation_cancellation_is_observed_before_sleep_parks() {
    let root = Context::new();
    let (fiber_handle, ctx) = scoped_ctx(&root).await;
    let sleep = ctx.sleep(Duration::from_secs(60)).unwrap();
    fiber_handle.dispose().await.unwrap();
    let before = tokio::time::Instant::now();
    assert!(matches!(sleep.await, Err(TimerCancelled)));
    assert_eq!(tokio::time::Instant::now(), before);
}

#[test]
fn zero_interval_precedes_inactive_context() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let ctx = rt.block_on(async {
        let root = Context::new();
        let (fiber_handle, ctx) = scoped_ctx(&root).await;
        fiber_handle.dispose().await.unwrap();
        ctx
    });
    assert!(matches!(
        ctx.interval(Duration::ZERO),
        Err(TimerRegistrationError::ZeroPeriod)
    ));
}

#[test]
fn unavailable_environment_precedes_deadline_range() {
    let ctx = Context::new();
    assert!(matches!(
        ctx.sleep(Duration::MAX),
        Err(TimerRegistrationError::TimerUnavailable)
    ));
    assert!(matches!(
        ctx.timeout(Duration::MAX, std::future::pending::<()>()),
        Err(TimerRegistrationError::TimerUnavailable)
    ));
}

#[test]
fn all_public_timer_constructors_refuse_without_time_driver() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let ctx = Context::new();
    let _guard = rt.enter();
    assert!(matches!(
        ctx.sleep(Duration::from_millis(1)),
        Err(TimerRegistrationError::TimerUnavailable)
    ));
    assert!(matches!(
        ctx.timeout(Duration::from_millis(1), std::future::pending::<()>()),
        Err(TimerRegistrationError::TimerUnavailable)
    ));
    assert!(matches!(
        ctx.interval(Duration::from_millis(1)),
        Err(TimerRegistrationError::TimerUnavailable)
    ));
}
