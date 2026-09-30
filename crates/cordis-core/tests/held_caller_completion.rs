//! Committed lifecycle work while the committing caller's future is alive but
//! no longer polled (issue #232, scan finding B1).
//!
//! ADR 0029: "Cancellation after that commit abandons only the caller's wait;
//! framework-owned work continues independently of caller polling until it
//! reaches the operation's documented barrier."
//!
//! Each test polls a committed lifecycle future once, past its irreversible
//! commit, then holds it without polling or dropping it. That is what the
//! unfinished future returned by `futures::future::select`, or a pinned future
//! parked in a struct, looks like. Disposal and typed removal must still reach
//! their barriers, and calls that coalesce onto them must not wait for the held
//! future. Creation is the documented exception: while its spawn future is
//! alive, creation advances only as that future is polled, and dropping the
//! future hands the committed creation to framework completion.

use cordis_core::lifecycle::FiberRole;
use cordis_core::{Context, FiberState, Plugin, PreparedPlugin};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

/// Registers one async cleanup gated by the test. The terminal drain runs it on
/// Cordis's completion runtime and awaits its outcome, so the committed owner
/// deterministically returns `Pending` until the gate opens.
struct AsyncCleanup(Arc<tokio::sync::Notify>);

impl Plugin for AsyncCleanup {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _: &()) -> Result<(), Infallible> {
        let gate = self.0.clone();
        ctx.effect(move || async move { gate.notified().await })
            .expect("Loading admits cleanup");
        Ok(())
    }
}

/// Apply parks until released, so the spawn future returns `Pending` after the
/// creation commit (allocation and Registry attach), with the initial settle
/// driven inline by that future.
struct ParkedApply(Arc<tokio::sync::Notify>);

impl Plugin for ParkedApply {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, _: Context, _: &()) -> Result<(), Infallible> {
        self.0.notified().await;
        Ok(())
    }
}

const BARRIER: Duration = Duration::from_secs(5);

fn ordinary_residents(ctx: &Context) -> usize {
    ctx.runtime_snapshot()
        .fibers()
        .iter()
        .filter(|fiber| fiber.role() == FiberRole::Ordinary)
        .count()
}

async fn committed_dispose_progresses_while_first_caller_future_is_held() {
    let ctx = Context::new();
    let gate = Arc::new(tokio::sync::Notify::new());
    let handle = ctx
        .spawn(PreparedPlugin::from_input(AsyncCleanup(gate.clone()), ()))
        .await
        .unwrap();

    // One poll commits Open-to-Closing and drives teardown until the gated
    // async cleanup's outcome is pending.
    let mut first = Box::pin(handle.dispose());
    assert!(futures::poll!(first.as_mut()).is_pending());
    assert_eq!(handle.state(), FiberState::Unloading);
    gate.notify_one();

    // `first` stays alive and unpolled. Coalescing calls wait only for the
    // committed owner's barrier.
    let second = tokio::time::timeout(BARRIER, handle.dispose()).await;
    let state = handle.state();
    assert!(
        matches!(second, Ok(Ok(()))),
        "a coalescing dispose must not wait for the held first caller \
         (state={state:?}, result={second:?})"
    );
    assert_eq!(state, FiberState::Disposed);
    assert_eq!(ordinary_residents(&ctx), 0);
    let ready = tokio::time::timeout(BARRIER, handle.ready()).await;
    assert!(
        matches!(ready, Ok(Ok(FiberState::Disposed))),
        "ready() must not wait for the held first caller: {ready:?}"
    );

    // Resuming the held future only observes the finished barrier.
    let first = tokio::time::timeout(BARRIER, first).await;
    assert!(matches!(first, Ok(Ok(()))), "{first:?}");
}

async fn committed_group_removal_progresses_while_caller_future_is_held() {
    let ctx = Context::new();
    let gate = Arc::new(tokio::sync::Notify::new());
    let handle = ctx
        .spawn(PreparedPlugin::from_input(AsyncCleanup(gate.clone()), ()))
        .await
        .unwrap();

    // One poll detaches the allocation, the removal's irreversible commit, and
    // drives the frozen drain until the member's gated async cleanup is pending.
    let mut removal = Box::pin(ctx.remove_plugins::<AsyncCleanup>());
    assert!(futures::poll!(removal.as_mut()).is_pending());
    gate.notify_one();

    // A direct dispose of the frozen member coalesces onto the removal's
    // committed owner, which must not depend on the held removal future.
    let member = tokio::time::timeout(BARRIER, handle.dispose()).await;
    let state = handle.state();
    assert!(
        matches!(member, Ok(Ok(()))),
        "committed typed removal must dispose its frozen member while the removing \
         caller's future is held (state={state:?}, result={member:?})"
    );
    assert_eq!(state, FiberState::Disposed);
    assert_eq!(ordinary_residents(&ctx), 0);

    // A later removal sees no allocation, and resuming the held removal only
    // observes the finished drain.
    let later = tokio::time::timeout(BARRIER, ctx.remove_plugins::<AsyncCleanup>()).await;
    assert!(matches!(later, Ok(Ok(()))), "{later:?}");
    let resumed = tokio::time::timeout(BARRIER, removal).await;
    assert!(matches!(resumed, Ok(Ok(()))), "{resumed:?}");
}

/// Documented narrowing: a held spawn future keeps its committed creation, and
/// what waits on it, where it is. Dropping the future hands the creation to
/// framework completion.
async fn held_spawn_future_advances_creation_only_as_it_is_polled() {
    let ctx = Context::new();
    let gate = Arc::new(tokio::sync::Notify::new());
    let mut spawn = Box::pin(ctx.spawn(PreparedPlugin::from_input(ParkedApply(gate.clone()), ())));
    assert!(futures::poll!(spawn.as_mut()).is_pending());
    assert_eq!(
        ordinary_residents(&ctx),
        1,
        "creation committed its allocation"
    );

    // The apply may now finish, but only the held spawn future drives the
    // initial settle, so a typed removal of the same allocation waits for it.
    gate.notify_one();
    let removal_ctx = ctx.clone();
    let removal = tokio::spawn(async move { removal_ctx.remove_plugins::<ParkedApply>().await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !removal.is_finished(),
        "a held spawn future does not advance its creation"
    );
    let states: Vec<FiberState> = ctx
        .runtime_snapshot()
        .fibers()
        .iter()
        .filter(|fiber| fiber.role() == FiberRole::Ordinary)
        .map(|fiber| fiber.state())
        .collect();
    assert_eq!(states, [FiberState::Loading]);

    // Dropping the held future hands the committed creation to framework
    // completion, and the removal waiting on it finishes.
    drop(spawn);
    let removal = tokio::time::timeout(BARRIER, removal).await;
    assert!(
        matches!(removal, Ok(Ok(Ok(())))),
        "typed removal completes once the spawn future is dropped: {removal:?}"
    );
    assert_eq!(ordinary_residents(&ctx), 0);
}

macro_rules! on_both_flavors {
    ($($name:ident),* $(,)?) => {
        mod multi_thread {
            $(
                #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
                async fn $name() {
                    super::$name().await;
                }
            )*
        }

        mod current_thread {
            $(
                #[tokio::test(flavor = "current_thread")]
                async fn $name() {
                    super::$name().await;
                }
            )*
        }
    };
}

on_both_flavors!(
    committed_dispose_progresses_while_first_caller_future_is_held,
    committed_group_removal_progresses_while_caller_future_is_held,
    held_spawn_future_advances_creation_only_as_it_is_polled,
);
