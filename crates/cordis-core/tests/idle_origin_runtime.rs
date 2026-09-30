//! Committed lifecycle work must reach its barrier without depending on the
//! caller's polling (ADR 0029) even when the runtime that committed it is a
//! `current_thread` runtime that is alive but no longer being driven.
//!
//! Since 0.6 that law is narrowed for new-Fiber creation: creation up to
//! FiberHandle delivery is driven by polling the spawn future, so it does not
//! advance on its own while that future is held, and dropping the future hands
//! the committed creation to framework completion. Disposal and Registry
//! removal are handed to framework completion at their first `Pending`
//! regardless of the caller.
//!
//! Public API only. Each test commits one operation on an origin current-thread
//! runtime and leaves that runtime alive and idle, then waits for the
//! documented barrier from an independent multi-thread runtime. Two shapes:
//!
//! - Abandoned callers: the caller's wait is dropped after the commit, possibly
//!   after first holding the once-polled future across an assertion of the
//!   committed state (as the creation rollback test does for residency).
//! - Held callers: the caller's future is polled once, then kept alive without
//!   being polled again, so only work already handed to framework completion
//!   can make progress.

use cordis_core::{Context, FiberState, Plugin, PreparedPlugin};
use futures::FutureExt;
use std::convert::Infallible;
use std::time::Duration;

/// Registers one async cleanup that parks until released, so a committed
/// disposal owner is still pending when its caller abandons the wait.
struct ParkedCleanup(std::sync::Arc<tokio::sync::Notify>);

impl Plugin for ParkedCleanup {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _input: &()) -> Result<(), Infallible> {
        let release = self.0.clone();
        let _cleanup = ctx
            .effect(move || async move { release.notified().await })
            .unwrap();
        Ok(())
    }
}

fn origin_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn observer_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

fn ordinary_residents(ctx: &Context) -> usize {
    ctx.runtime_snapshot()
        .fibers()
        .iter()
        .filter(|fiber| fiber.role() == cordis_core::lifecycle::FiberRole::Ordinary)
        .count()
}

/// Bounds only failure: a generous ceiling so a loaded CI runner cannot flake a
/// wait that otherwise completes in milliseconds.
const BARRIER_TIMEOUT: Duration = Duration::from_secs(10);

/// Polls until no ordinary Fiber is resident, or reports `false` once
/// `BARRIER_TIMEOUT` elapses. Observes only; it never drives a Fiber itself.
async fn wait_until_no_ordinary_residents(ctx: &Context) -> bool {
    tokio::time::timeout(BARRIER_TIMEOUT, async {
        while ordinary_residents(ctx) != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok()
}

#[test]
fn committed_dispose_completes_while_the_origin_current_thread_runtime_is_idle() {
    let origin = origin_runtime();
    let ctx = Context::new();
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let handle = origin
        .block_on(ctx.spawn(PreparedPlugin::from_input(
            ParkedCleanup(release.clone()),
            (),
        )))
        .unwrap();

    // First poll commits the Open-to-Closing claim; the caller then abandons
    // only its wait, exactly as a `select!`/timeout would.
    origin.block_on(async {
        assert!(handle.dispose().now_or_never().is_none());
    });
    assert!(
        ordinary_residents(&ctx) == 1,
        "the committed owner is still pending"
    );

    // The origin runtime stays alive but is not driven again.
    let observer = observer_runtime();
    let completed = observer.block_on(async {
        release.notify_one();
        tokio::time::timeout(BARRIER_TIMEOUT, handle.dispose())
            .await
            .is_ok()
    });
    let state = handle.state();
    let residents = ordinary_residents(&ctx);
    drop(observer);
    drop(origin);
    assert!(
        completed,
        "committed dispose did not reach its barrier while the origin runtime was idle \
         (state={state:?}, ordinary residents={residents})"
    );
    assert_eq!(state, FiberState::Disposed);
    assert_eq!(residents, 0);
}

#[test]
fn committed_group_removal_completes_while_the_origin_current_thread_runtime_is_idle() {
    let origin = origin_runtime();
    let ctx = Context::new();
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let handle = origin
        .block_on(ctx.spawn(PreparedPlugin::from_input(
            ParkedCleanup(release.clone()),
            (),
        )))
        .unwrap();

    // First poll detaches the allocation (the irreversible commit).
    origin.block_on(async {
        assert!(
            ctx.remove_plugins::<ParkedCleanup>()
                .now_or_never()
                .is_none()
        );
    });
    release.notify_one();

    // Observe only: a second dispose() would itself drive the member and mask
    // whether the removal's own framework owner made progress.
    let observer = observer_runtime();
    let completed = observer.block_on(wait_until_no_ordinary_residents(&ctx));
    let state = handle.state();
    let residents = ordinary_residents(&ctx);
    drop(observer);
    drop(origin);
    assert!(
        completed,
        "committed group removal did not dispose its frozen member while the origin runtime \
         was idle (state={state:?}, ordinary residents={residents})"
    );
}

#[test]
fn abandoned_creation_is_rolled_back_while_the_origin_current_thread_runtime_is_idle() {
    struct Parked(std::sync::Arc<tokio::sync::Notify>);
    impl Plugin for Parked {
        type Config = ();
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = Infallible;
        fn prepare(&self, (): ()) -> Result<(), Infallible> {
            Ok(())
        }
        async fn apply(&self, _ctx: Context, _input: &()) -> Result<(), Infallible> {
            self.0.notified().await;
            Ok(())
        }
    }

    let origin = origin_runtime();
    let ctx = Context::new();
    let never = std::sync::Arc::new(tokio::sync::Notify::new());
    // First poll commits allocation and starts the initial apply, which parks.
    // The future is held across the residency assertion: dropping it starts
    // the rollback on the completion runtime, which can finish before an
    // assertion made after the drop (issue #238).
    let mut spawn = Box::pin(ctx.spawn(PreparedPlugin::from_input(Parked(never), ())));
    origin.block_on(async {
        assert!(futures::poll!(spawn.as_mut()).is_pending());
    });
    assert_eq!(
        ordinary_residents(&ctx),
        1,
        "creation committed its allocation"
    );

    // The caller then abandons the creation on the origin runtime, which is
    // not driven again. The drop deliberately runs inside `origin.block_on`, so
    // `detach` sees a current-thread runtime handle and starts the rollback on
    // the completion runtime (its `Ok(_)` arm); the held variant below drops
    // off-runtime (its `Err(_)` arm), which reaches the same target.
    origin.block_on(async { drop(spawn) });

    // Bounded wait for the rollback.
    let observer = observer_runtime();
    let rolled_back = observer.block_on(wait_until_no_ordinary_residents(&ctx));
    let residents = ordinary_residents(&ctx);
    drop(observer);
    drop(origin);
    assert!(
        rolled_back,
        "abandoned creation was not rolled back while the origin runtime was idle \
         (ordinary residents={residents})"
    );
}

// Held-caller variants (issue #232). The caller's future is polled once on the
// idle origin and then kept alive without being polled or dropped, as the
// unfinished half of a `select` would be. Committed disposal and removal must
// still finish; creation advances only once that future is dropped.

#[test]
fn committed_dispose_completes_while_its_held_caller_and_the_origin_runtime_are_idle() {
    let origin = origin_runtime();
    let ctx = Context::new();
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let handle = origin
        .block_on(ctx.spawn(PreparedPlugin::from_input(
            ParkedCleanup(release.clone()),
            (),
        )))
        .unwrap();

    let mut first = Box::pin(handle.dispose());
    origin.block_on(async {
        assert!(futures::poll!(first.as_mut()).is_pending());
    });

    let observer = observer_runtime();
    let completed = observer.block_on(async {
        release.notify_one();
        tokio::time::timeout(BARRIER_TIMEOUT, handle.dispose())
            .await
            .is_ok()
    });
    let state = handle.state();
    let residents = ordinary_residents(&ctx);
    assert!(
        completed,
        "committed dispose did not reach its barrier while its caller future was held on \
         the idle origin runtime (state={state:?}, ordinary residents={residents})"
    );
    assert_eq!(state, FiberState::Disposed);
    assert_eq!(residents, 0);

    // The held future only observes the finished barrier once resumed.
    assert!(origin.block_on(first).is_ok());
    drop(observer);
    drop(origin);
}

#[test]
fn committed_group_removal_completes_while_its_held_caller_and_the_origin_runtime_are_idle() {
    let origin = origin_runtime();
    let ctx = Context::new();
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let handle = origin
        .block_on(ctx.spawn(PreparedPlugin::from_input(
            ParkedCleanup(release.clone()),
            (),
        )))
        .unwrap();

    let mut removal = Box::pin(ctx.remove_plugins::<ParkedCleanup>());
    origin.block_on(async {
        assert!(futures::poll!(removal.as_mut()).is_pending());
    });
    release.notify_one();

    // Observe only, as in the dropped-caller variant above.
    let observer = observer_runtime();
    let completed = observer.block_on(wait_until_no_ordinary_residents(&ctx));
    let state = handle.state();
    let residents = ordinary_residents(&ctx);
    assert!(
        completed,
        "committed group removal did not dispose its frozen member while its caller future \
         was held on the idle origin runtime (state={state:?}, ordinary residents={residents})"
    );
    assert_eq!(state, FiberState::Disposed);

    assert!(origin.block_on(removal).is_ok());
    drop(observer);
    drop(origin);
}

/// Documented narrowing: a held spawn future keeps its committed creation
/// where it is. Dropping it, even without driving the origin again, hands the
/// rollback to framework completion.
#[test]
fn held_creation_is_rolled_back_once_its_future_is_dropped_while_the_origin_runtime_is_idle() {
    struct Parked(std::sync::Arc<tokio::sync::Notify>);
    impl Plugin for Parked {
        type Config = ();
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = Infallible;
        fn prepare(&self, (): ()) -> Result<(), Infallible> {
            Ok(())
        }
        async fn apply(&self, _ctx: Context, _input: &()) -> Result<(), Infallible> {
            self.0.notified().await;
            Ok(())
        }
    }

    let origin = origin_runtime();
    let ctx = Context::new();
    let never = std::sync::Arc::new(tokio::sync::Notify::new());
    let mut spawn = Box::pin(ctx.spawn(PreparedPlugin::from_input(Parked(never), ())));
    origin.block_on(async {
        assert!(futures::poll!(spawn.as_mut()).is_pending());
    });

    let observer = observer_runtime();
    observer.block_on(async { tokio::time::sleep(Duration::from_millis(100)).await });
    assert_eq!(
        ordinary_residents(&ctx),
        1,
        "a held spawn future keeps its committed creation"
    );

    drop(spawn);
    let rolled_back = observer.block_on(wait_until_no_ordinary_residents(&ctx));
    let residents = ordinary_residents(&ctx);
    drop(observer);
    drop(origin);
    assert!(
        rolled_back,
        "dropped creation was not rolled back while the origin runtime was idle \
         (ordinary residents={residents})"
    );
}
