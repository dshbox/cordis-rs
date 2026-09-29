//! Committed lifecycle work must reach its barrier
//! "independently of caller polling" (ADR 0029) even when the runtime that
//! committed it is a `current_thread` runtime that is alive but no longer
//! being driven.
//!
//! Public API only. Each test commits one operation on an origin
//! current-thread runtime, abandons the caller's wait immediately after the
//! commit (one poll), leaves the origin runtime alive and idle, then waits for
//! the documented barrier from an independent multi-thread runtime.

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
        tokio::time::timeout(Duration::from_secs(3), handle.dispose())
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
    let completed = observer.block_on(async {
        tokio::time::timeout(Duration::from_secs(3), async {
            while ordinary_residents(&ctx) != 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok()
    });
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
    // First poll commits allocation and starts the initial apply, which parks;
    // the caller then abandons the creation.
    origin.block_on(async {
        let never = std::sync::Arc::new(tokio::sync::Notify::new());
        assert!(
            ctx.spawn(PreparedPlugin::from_input(Parked(never), ()))
                .now_or_never()
                .is_none()
        );
    });
    assert_eq!(
        ordinary_residents(&ctx),
        1,
        "creation committed its allocation"
    );

    let observer = observer_runtime();
    let rolled_back = observer.block_on(async {
        tokio::time::timeout(Duration::from_secs(3), async {
            while ordinary_residents(&ctx) != 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok()
    });
    let residents = ordinary_residents(&ctx);
    drop(observer);
    drop(origin);
    assert!(
        rolled_back,
        "abandoned creation was not rolled back while the origin runtime was idle \
         (ordinary residents={residents})"
    );
}
