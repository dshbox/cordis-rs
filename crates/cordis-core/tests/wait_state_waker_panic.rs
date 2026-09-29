//! A caller's panicking `Waker` cannot stop deadline delivery to other waits.
//!
//! Every `wait_state` deadline is fired by one shared scheduler thread, which
//! wakes the waiting task. That wake runs a caller-supplied `Waker`. If it
//! panics, only that caller's wake is lost: other callers' deadlines must still
//! elapse on time.

mod common;

use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll, Wake, Waker};
use std::time::Duration;

use common::bounded;
use cordis_core::lifecycle::WaitStateError;
use cordis_core::{Context, FiberState, Plugin, PreparedPlugin};

struct Idle;

impl Plugin for Idle {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, _ctx: Context, _: &()) -> Result<(), Infallible> {
        Ok(())
    }
}

struct PanickingWaker;

impl Wake for PanickingWaker {
    fn wake(self: Arc<Self>) {
        panic!("caller waker panicked on deadline wake");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_panicking_waker_does_not_stop_other_deadlines() {
    let ctx = Context::new();
    let fiber = ctx
        .spawn(PreparedPlugin::from_input(Idle, ()))
        .await
        .unwrap();

    // Park one wait under a Waker that panics when its deadline fires.
    const POISONED: Duration = Duration::from_millis(400);
    const ORDINARY: Duration = Duration::from_millis(800);
    let waker = Waker::from(Arc::new(PanickingWaker));
    let poisoned_armed = std::time::Instant::now();
    let mut poisoned = Box::pin(fiber.wait_state(FiberState::Pending, POISONED));
    assert!(matches!(
        poisoned.as_mut().poll(&mut TaskContext::from_waker(&waker)),
        Poll::Pending
    ));

    // Arm an ordinary wait while the poisoned deadline is still pending. The
    // precondition is checked, not assumed: arming after the poisoned wake
    // would start a fresh worker and could not observe the regression.
    let ordinary_armed = std::time::Instant::now();
    let ordinary = fiber.wait_state(FiberState::Pending, ORDINARY);
    tokio::pin!(ordinary);
    assert!(futures::poll!(&mut ordinary).is_pending());
    assert!(
        poisoned_armed.elapsed() < POISONED,
        "precondition: the ordinary wait must be armed before the poisoned wake fires"
    );

    let ordinary = bounded(2_000, ordinary)
        .await
        .expect("a panicking Waker stopped delivery of another caller's deadline");
    assert!(matches!(ordinary, Err(WaitStateError::Elapsed)));
    assert!(ordinary_armed.elapsed() >= ORDINARY);

    // Later arms are served too.
    let later = bounded(
        1_000,
        fiber.wait_state(FiberState::Pending, Duration::from_millis(20)),
    )
    .await
    .expect("the deadline scheduler kept serving later waits");
    assert!(matches!(later, Err(WaitStateError::Elapsed)));

    // Drop the poisoned wait before teardown publishes states, so its
    // deliberately panicking Waker is not woken by disposal.
    drop(poisoned);
    fiber.dispose().await.unwrap();
}
