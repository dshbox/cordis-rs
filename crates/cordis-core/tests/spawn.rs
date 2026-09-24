//! Spawn contract: `Context::spawn(PreparedPlugin)` performs the whole
//! creation transaction and delivers a [`FiberHandle`] only for a live quiescent
//! Fiber — `Active`, or stable `Pending` while required services are
//! missing (LF-01). An initial apply that returns an error or panics runs
//! its complete LIFO rollback, leaves no resident attempted Fiber, and
//! reports one normalized [`PluginFailure`] through
//! [`SpawnError::InitialApply`] (LF-07); caller cancellation after the
//! allocation commit completes the disposal and unlink under framework
//! ownership, independently of caller polling (LF-19). Preparation panics
//! stay ordinary pre-admission unwinds: they allocate nothing and are
//! never normalized (LF-07's boundary).
//!
//! The `SpawnError::Interrupted` arm (framework invalidation before
//! handoff) is covered at the unit tier
//! (`fiber::inertia::tests::initial_spawn_pass_on_an_invalidated_fiber_reports_interrupted`):
//! the pre-claim section of a spawn is yield-free, so the invalidation
//! race cannot be sequenced deterministically through the public API.

mod common;

use common::{bounded, ordinary_fiber_count};
use cordis_core::lifecycle::{PluginFailureKind, SpawnError};
use cordis_core::{Context, FiberState, InjectSpec, Plugin, PreparedPlugin, Service};
use parking_lot::Mutex;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// The concrete apply-failure carrier for this laboratory (the typed
/// error proves the diagnostic survives normalization verbatim).
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct ApplyBoom(String);

/// A plugin whose apply body is the closure it carries.
struct Scripted<F>(F);

impl<F> Plugin for Scripted<F>
where
    F: Fn(Context) -> Result<(), ApplyBoom> + Send + Sync + 'static,
{
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = ApplyBoom;

    fn prepare(&self, _config: ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), ApplyBoom> {
        (self.0)(ctx)
    }
}

/// The service the dependency tests rendezvous on; `uses` counts how
/// often a dependent applied against it.
#[derive(Default)]
struct Counter {
    uses: AtomicU32,
}

impl Service for Counter {
    const NAME: &'static str = "counter";
}

/// A one-shot barrier that can live behind an `Fn`/`&self` apply: the
/// take happens through the mutex, so the apply stays reusable.
type BarrierSlot<T> = Arc<Mutex<Option<T>>>;

// ---------------------------------------------------------------------------
// LF-07: an initial apply that returns an error closes the generation,
// runs every committed cleanup in LIFO order, withdraws its publications,
// and reports one normalized PluginFailure — and LF-01: no resident
// attempted Fiber survives the failure.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn initial_apply_error_rolls_back_lifo_and_leaves_no_resident_fiber() {
    let ctx = Context::new();
    let order: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));

    let order_probe = order.clone();
    let err = ctx
        .spawn(PreparedPlugin::from_input(
            Scripted(move |ctx: Context| -> Result<(), ApplyBoom> {
                ctx.effect_sync({
                    let order_probe = order_probe.clone();
                    move || order_probe.lock().push("first")
                })
                .unwrap();
                let _ = ctx
                    .provide::<Counter>(Arc::new(Counter::default()))
                    .unwrap();
                ctx.effect_sync({
                    let order_probe = order_probe.clone();
                    move || order_probe.lock().push("last")
                })
                .unwrap();
                Err(ApplyBoom("apply boom".to_owned()))
            }),
            (),
        ))
        .await
        .expect_err("a failed initial apply refuses the FiberHandle");

    let SpawnError::InitialApply(failure) = err else {
        panic!("expected InitialApply, got {err:?}");
    };
    assert_eq!(failure.kind(), PluginFailureKind::ReturnedError);
    assert_eq!(failure.diagnostic(), "apply boom");

    // complete LIFO rollback: the last registration ran first, the
    // publication's withdrawal rode the same drain...
    assert_eq!(*order.lock(), ["last", "first"]);
    // ...so the publication is gone with the generation
    assert!(
        ctx.try_service::<Counter>().is_err(),
        "a rolled-back publication is withdrawn"
    );
    // no resident attempted Fiber survives the completed failure barrier
    assert_eq!(ordinary_fiber_count(&ctx), 0);
}

#[tokio::test]
async fn initial_apply_panic_rolls_back_and_leaves_no_resident_fiber() {
    struct Panicky;
    impl Plugin for Panicky {
        type Config = ();
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = ApplyBoom;

        fn prepare(&self, _config: ()) -> Result<(), Infallible> {
            Ok(())
        }

        async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), ApplyBoom> {
            ctx.effect_sync(|| -> () { panic!("the rollback cleanup stays silent") })
                .unwrap();
            panic!("apply exploded");
        }
    }

    let ctx = Context::new();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = ctx.spawn(PreparedPlugin::from_input(Panicky, ())).await;
    std::panic::set_hook(default_hook);

    let err = outcome.expect_err("a panicking initial apply refuses the FiberHandle");
    let SpawnError::InitialApply(failure) = err else {
        panic!("expected InitialApply, got {err:?}");
    };
    assert_eq!(failure.kind(), PluginFailureKind::Panic);
    assert_eq!(failure.diagnostic(), "apply exploded");
    assert_eq!(ordinary_fiber_count(&ctx), 0);
}

// ---------------------------------------------------------------------------
// LF-07's boundary: preparation panics are ordinary pre-admission unwinds —
// they allocate no Runtime state and are never normalized into a
// PluginFailure.
// ---------------------------------------------------------------------------

#[test]
fn preparation_panic_is_an_ordinary_unwind_before_admission() {
    struct PanickyPrepare;
    impl Plugin for PanickyPrepare {
        type Config = ();
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = ApplyBoom;

        fn name(&self) -> std::borrow::Cow<'_, str> {
            panic!("name exploded")
        }

        fn prepare(&self, _config: ()) -> Result<(), Infallible> {
            panic!("prepare exploded")
        }

        async fn apply(&self, _ctx: Context, _prepared: &()) -> Result<(), ApplyBoom> {
            Ok(())
        }
    }

    let ctx = Context::new();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let prepare_outcome =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| PanickyPrepare.prepare(())));
    let seal_outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        PreparedPlugin::from_input(PanickyPrepare, ())
    }));
    std::panic::set_hook(default_hook);

    assert_eq!(
        prepare_outcome.unwrap_err().downcast_ref::<&str>(),
        Some(&"prepare exploded"),
        "prepare's panic unwinds to the caller untouched"
    );
    assert_eq!(
        seal_outcome
            .err()
            .and_then(|payload| payload.downcast_ref::<&str>().copied()),
        Some("name exploded"),
        "sealing's declaration materialization unwinds untouched"
    );

    // checked against a runtime that already existed: the unwinds
    // neither allocated nor mutated anything on it
    assert_eq!(
        ordinary_fiber_count(&ctx),
        0,
        "pre-admission unwinds allocate nothing"
    );
}
// ---------------------------------------------------------------------------
// LF-01: an eligible creation settles Pending → Loading → Active and the
// delivered FiberHandle belongs to a live quiescent fiber.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn eligible_spawn_delivers_a_live_quiescent_active_fiber_handle() {
    let ctx = Context::new();

    let applied = Arc::new(AtomicU32::new(0));
    let applied_probe = applied.clone();
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(
            Scripted(move |_ctx: Context| -> Result<(), ApplyBoom> {
                applied_probe.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }),
            (),
        ))
        .await
        .expect("an eligible spawn hands off its FiberHandle");

    assert_eq!(fiber_handle.state(), FiberState::Active);
    let _identity = fiber_handle.id();
    assert_eq!(ordinary_fiber_count(&ctx), 1);
    // Lifecycle transition narration belongs to Runtime observation; this
    // spawn contract proves only the live quiescent handoff.
    assert_eq!(applied.load(Ordering::SeqCst), 1, "apply ran exactly once");
}

// ---------------------------------------------------------------------------
// LF-01: missing requirements produce a stable Pending without apply —
// still a live quiescent handoff — and a later publication converges the
// same fiber.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn missing_requirements_hand_off_a_stable_pending_fiber_handle_without_applying() {
    struct Wants;
    impl Plugin for Wants {
        type Config = ();
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = ApplyBoom;

        fn inject(&self) -> InjectSpec {
            InjectSpec::none().require(Counter::NAME)
        }

        fn prepare(&self, _config: ()) -> Result<(), Infallible> {
            Ok(())
        }

        async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), ApplyBoom> {
            let counter = ctx
                .try_service::<Counter>()
                .map_err(|e| ApplyBoom(e.to_string()))?;
            counter.uses.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let ctx = Context::new();
    let counter = Arc::new(Counter::default());
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(Wants, ()))
        .await
        .expect("a Pending fiber is still a live quiescent handoff");
    assert_eq!(fiber_handle.state(), FiberState::Pending);
    assert_eq!(fiber_handle.pending_missing(), [Counter::NAME.to_owned()]);
    assert_eq!(
        counter.uses.load(Ordering::SeqCst),
        0,
        "apply never ran for the ineligible fiber"
    );
    // stable and quiescent: ready() answers immediately, nothing is
    // in flight
    assert_eq!(
        bounded(200, fiber_handle.ready())
            .await
            .expect("the Pending FiberHandle is quiescent at handoff")
            .unwrap(),
        FiberState::Pending
    );

    // the publication kicks the parked fiber: it converges onto the
    // provider and applies exactly once
    let _ = ctx.provide::<Counter>(counter.clone()).unwrap();
    assert_eq!(
        fiber_handle.ready().await.unwrap(),
        FiberState::Active,
        "the publication converged the parked fiber"
    );
    assert_eq!(counter.uses.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// LF-01: a service mutation racing the initial apply is converged before
// the handoff — the FiberHandle is delivered only once the fiber is quiescent
// for the current service snapshot, here: stable Pending over a
// requirement that disappeared mid-apply.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_mutation_racing_the_initial_apply_is_converged_before_handoff() {
    let ctx = Context::new();
    let counter = Arc::new(Counter::default());
    let provider = {
        let counter = counter.clone();
        ctx.spawn(PreparedPlugin::from_input(
            Scripted(move |ctx: Context| -> Result<(), ApplyBoom> {
                let _ = ctx
                    .provide::<Counter>(counter.clone())
                    .map_err(|e| ApplyBoom(e.to_string()))?;
                Ok(())
            }),
            (),
        ))
        .await
        .expect("the provider spawns")
    };

    // the dependent requires Counter; its first apply blocks until the
    // test has withdrawn the provider mid-apply
    let entered: BarrierSlot<tokio::sync::oneshot::Sender<()>> = Arc::new(Mutex::new(None));
    let go: BarrierSlot<tokio::sync::oneshot::Receiver<()>> = Arc::new(Mutex::new(None));
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (go_tx, go_rx) = tokio::sync::oneshot::channel();
    *entered.lock() = Some(entered_tx);
    *go.lock() = Some(go_rx);

    struct Dependent {
        entered: BarrierSlot<tokio::sync::oneshot::Sender<()>>,
        go: BarrierSlot<tokio::sync::oneshot::Receiver<()>>,
        applies: Arc<AtomicU32>,
        drained: Arc<AtomicU32>,
    }
    impl Plugin for Dependent {
        type Config = ();
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = ApplyBoom;

        fn inject(&self) -> InjectSpec {
            InjectSpec::none().require(Counter::NAME)
        }

        fn prepare(&self, _config: ()) -> Result<(), Infallible> {
            Ok(())
        }

        async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), ApplyBoom> {
            self.applies.fetch_add(1, Ordering::SeqCst);
            ctx.effect_sync({
                let drained = self.drained.clone();
                move || {
                    drained.fetch_add(1, Ordering::SeqCst);
                }
            })
            .map_err(|e| ApplyBoom(e.to_string()))?;
            let _ = self
                .entered
                .lock()
                .take()
                .expect("the barrier is armed once")
                .send(());
            let go = self.go.lock().take().expect("the barrier is armed once");
            let _ = go.await;
            Ok(())
        }
    }

    let applies = Arc::new(AtomicU32::new(0));
    let drained = Arc::new(AtomicU32::new(0));
    let spawn = tokio::spawn({
        let ctx = ctx.clone();
        let (applies, drained) = (applies.clone(), drained.clone());
        async move {
            ctx.spawn(PreparedPlugin::from_input(
                Dependent {
                    entered,
                    go,
                    applies,
                    drained,
                },
                (),
            ))
            .await
        }
    });
    bounded(2000, entered_rx)
        .await
        .expect("the dependent's apply is in flight")
        .unwrap();

    // withdraw the requirement mid-apply: the dependent's fingerprint
    // drifts to INACTIVE while its creation pass holds the settle slot
    provider.dispose().await.unwrap();
    go_tx.send(()).unwrap();

    // the handoff waits out the drift: the delivered FiberHandle is the
    // quiescent converged answer, stable Pending over the vanished
    // requirement — not the Active the first apply briefly reached
    let fiber_handle = bounded(2000, spawn)
        .await
        .expect("the creation completes")
        .unwrap()
        .expect("the raced withdrawal converges, not fails");
    assert_eq!(fiber_handle.state(), FiberState::Pending);
    assert_eq!(fiber_handle.pending_missing(), [Counter::NAME.to_owned()]);
    assert_eq!(
        applies.load(Ordering::SeqCst),
        1,
        "the converged Pending target is never re-applied"
    );
    assert_eq!(
        drained.load(Ordering::SeqCst),
        1,
        "the superseded generation drained before handoff"
    );
}

// ---------------------------------------------------------------------------
// SpawnError::InactiveContext: spawning through a closed generation is a
// pre-commit refusal — nothing is allocated.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_through_an_inactive_context_is_refused_before_allocation() {
    let ctx = Context::new();
    let captured: Arc<Mutex<Option<Context>>> = Arc::new(Mutex::new(None));
    let captured_probe = captured.clone();
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(
            Scripted(move |ctx: Context| -> Result<(), ApplyBoom> {
                *captured_probe.lock() = Some(ctx);
                Ok(())
            }),
            (),
        ))
        .await
        .expect("the first spawn hands off");
    fiber_handle.dispose().await.unwrap();

    let captured_ctx = captured.lock().clone().unwrap();
    let err = captured_ctx
        .spawn(PreparedPlugin::from_input(
            Scripted(|_ctx: Context| -> Result<(), ApplyBoom> { Ok(()) }),
            (),
        ))
        .await
        .expect_err("a disposed spawning context refuses");
    assert!(
        matches!(err, SpawnError::InactiveContext),
        "the refusal is the admission gate's: {err:?}"
    );
    assert_eq!(
        ordinary_fiber_count(&ctx),
        0,
        "the refusal allocated nothing"
    );
}

// ---------------------------------------------------------------------------
// LF-19: the pre-allocation part of spawn is yield-free, so cancelling before
// the commit is represented by dropping the unpolled future. It must allocate
// nothing and run no Plugin work.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn caller_cancellation_before_commit_allocates_nothing() {
    let ctx = Context::new();
    let applies = Arc::new(AtomicU32::new(0));
    let observed = applies.clone();
    let spawn = ctx.spawn(PreparedPlugin::from_input(
        Scripted(move |_ctx: Context| -> Result<(), ApplyBoom> {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }),
        (),
    ));

    drop(spawn);

    assert_eq!(applies.load(Ordering::SeqCst), 0);
    assert_eq!(
        ordinary_fiber_count(&ctx),
        0,
        "precommit caller cancellation must leave no attempted Fiber resident"
    );
}

// ---------------------------------------------------------------------------
// LF-19: caller cancellation after the allocation commit completes the
// disposal and unlink of the undelivered Fiber under framework ownership,
// independently of caller polling — and is never reported as Interrupted
// (no error is reported at all: nobody is left to receive one).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn caller_cancellation_after_commit_completes_disposal_and_unlink() {
    let ctx = Context::new();
    let entered: BarrierSlot<tokio::sync::oneshot::Sender<()>> = Arc::new(Mutex::new(None));
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    *entered.lock() = Some(entered_tx);
    let rolled_back = Arc::new(AtomicU32::new(0));

    struct Blocking {
        entered: BarrierSlot<tokio::sync::oneshot::Sender<()>>,
        rolled_back: Arc<AtomicU32>,
    }
    impl Plugin for Blocking {
        type Config = ();
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = ApplyBoom;

        fn prepare(&self, _config: ()) -> Result<(), Infallible> {
            Ok(())
        }

        async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), ApplyBoom> {
            // a committed registration before the block: the guard's
            // teardown must drain it
            ctx.effect_sync({
                let rb = self.rolled_back.clone();
                move || {
                    rb.fetch_add(1, Ordering::SeqCst);
                }
            })
            .map_err(|e| ApplyBoom(e.to_string()))?;
            let _ = self
                .entered
                .lock()
                .take()
                .expect("the barrier is armed once")
                .send(());
            // never resolve: the cancellation lands mid-apply
            std::future::pending::<()>().await;
            Ok(())
        }
    }

    let spawn = tokio::spawn({
        let ctx = ctx.clone();
        let rb = rolled_back.clone();
        async move {
            ctx.spawn(PreparedPlugin::from_input(
                Blocking {
                    entered,
                    rolled_back: rb,
                },
                (),
            ))
            .await
        }
    });
    bounded(2000, entered_rx)
        .await
        .expect("the apply is in flight with the cleanup committed")
        .unwrap();

    // abandon the creation: the pass dies holding the settle slot, and
    // the creation guard completes the teardown framework-side
    spawn.abort();
    assert!(
        spawn.await.unwrap_err().is_cancelled(),
        "the caller cancelled; nothing is reported to it"
    );

    bounded(2000, async {
        loop {
            if rolled_back.load(Ordering::SeqCst) == 1 && ordinary_fiber_count(&ctx) == 0 {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the guard drained the generation and unlinked the fiber");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn caller_cancellation_mid_cleanup_completes_the_claimed_cleanup() {
    // LF-07's attempt-and-complete law against the claim-then-await
    // window: the rollback drain has claimed the cleanup and the cleanup
    // is parked mid-execution when the caller cancels. The claimed
    // cleanup must still run to completion under framework ownership —
    // dropping the spawn future must not destroy it.
    let ctx = Context::new();
    let entered: BarrierSlot<tokio::sync::oneshot::Sender<()>> = Arc::new(Mutex::new(None));
    let release: BarrierSlot<tokio::sync::oneshot::Receiver<()>> = Arc::new(Mutex::new(None));
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    *entered.lock() = Some(entered_tx);
    *release.lock() = Some(release_rx);
    let cleanup_done = Arc::new(AtomicU32::new(0));
    let done_probe = cleanup_done.clone();

    let plugin = Scripted(move |ctx: Context| -> Result<(), ApplyBoom> {
        ctx.effect({
            let entered = entered.clone();
            let release = release.clone();
            let done = done_probe.clone();
            move || {
                let entered = entered.lock().take().expect("armed once");
                let release = release.lock().take().expect("armed once");
                async move {
                    let _ = entered.send(());
                    let _ = release.await;
                    done.fetch_add(1, Ordering::SeqCst);
                }
            }
        })
        .map_err(|e| ApplyBoom(e.to_string()))?;
        Err(ApplyBoom("apply boom".to_owned()))
    });
    let spawn = tokio::spawn({
        let ctx = ctx.clone();
        async move { ctx.spawn(PreparedPlugin::from_input(plugin, ())).await }
    });
    bounded(2000, entered_rx)
        .await
        .expect("the rollback's cleanup is claimed and in flight")
        .unwrap();

    // abandon the creation mid-cleanup: the claimed cleanup is already
    // detached — the cancellation destroys only the drain's wait
    spawn.abort();
    assert!(spawn.await.unwrap_err().is_cancelled());

    release_tx.send(()).unwrap();
    bounded(2000, async {
        loop {
            if cleanup_done.load(Ordering::SeqCst) == 1 && ordinary_fiber_count(&ctx) == 0 {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the claimed cleanup completed and the fiber unlinked");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_creation_keeps_rollback_order_and_terminal_barrier() {
    let ctx = Context::new();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let older_ran = Arc::new(AtomicU32::new(0));
    let newer_done = Arc::new(AtomicU32::new(0));
    let plugin = Scripted({
        let entered = entered.clone();
        let release = release.clone();
        let older_ran = older_ran.clone();
        let newer_done = newer_done.clone();
        move |ctx: Context| -> Result<(), ApplyBoom> {
            let older_ran = older_ran.clone();
            ctx.effect_sync(move || {
                older_ran.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();
            let entered = entered.clone();
            let release = release.clone();
            let newer_done = newer_done.clone();
            ctx.effect(move || async move {
                entered.notify_one();
                release.notified().await;
                newer_done.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();
            Err(ApplyBoom("rollback".to_owned()))
        }
    });
    let spawn = tokio::spawn({
        let ctx = ctx.clone();
        async move { ctx.spawn(PreparedPlugin::from_input(plugin, ())).await }
    });
    entered.notified().await;
    spawn.abort();
    assert!(spawn.await.unwrap_err().is_cancelled());

    // The detached creation guard must join the cleanup already claimed by
    // the aborted drain before it can run the older cleanup or unlink.
    let premature = bounded(200, async {
        loop {
            if older_ran.load(Ordering::SeqCst) != 0 || ordinary_fiber_count(&ctx) == 0 {
                return true;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or(false);
    release.notify_one();
    bounded(2000, async {
        loop {
            if newer_done.load(Ordering::SeqCst) == 1
                && older_ran.load(Ordering::SeqCst) == 1
                && ordinary_fiber_count(&ctx) == 0
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("rollback completed after release");
    assert!(
        !premature,
        "rollback ran an older cleanup or unlinked before the claimed cleanup finished"
    );
}

/// Drive one poll of a future whose pending state the test then abandons
/// (off-runtime spawn-future drop below).
fn poll_once<F: Future>(fut: std::pin::Pin<&mut F>) -> std::task::Poll<F::Output> {
    let mut task_cx = std::task::Context::from_waker(std::task::Waker::noop());
    fut.poll(&mut task_cx)
}

#[test]
fn off_runtime_cancellation_drives_the_rollback_under_a_runtime() {
    // The creation guard's off-runtime arm must give the teardown a Tokio
    // runtime: a cleanup that legitimately spawns onto Tokio would panic
    // inside a runtime-less block_on — a framework-induced failure of
    // user code, breaking the postcommit-completion law (ADR 0029).
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let release_rx = Arc::new(Mutex::new(Some(release_rx)));

    let ctx = Context::new();
    let plugin = Scripted(move |ctx: Context| -> Result<(), ApplyBoom> {
        let entered_tx = entered_tx.clone();
        let done_tx = done_tx.clone();
        let release_rx = release_rx.clone();
        ctx.effect(move || {
            let release_rx = release_rx.lock().take().expect("armed once");
            async move {
                let _ = entered_tx.send(());
                let _ = release_rx.await;
                // the proof a Tokio runtime is current off-runtime: spawn
                // and join on it (panics inside a runtime-less block_on)
                tokio::spawn(async move {}).await.unwrap();
                let _ = done_tx.send(());
            }
        })
        .map_err(|e| ApplyBoom(e.to_string()))?;
        Err(ApplyBoom("apply boom".to_owned()))
    });
    let mut spawn = Box::pin(ctx.spawn(PreparedPlugin::from_input(plugin, ())));
    assert!(
        poll_once(spawn.as_mut()).is_pending(),
        "parked inside the rollback's blocking cleanup"
    );
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the cleanup started on the framework's own thread");

    // dropped off-runtime: the guard drives the teardown on a dedicated
    // thread carrying its own runtime
    drop(spawn);
    release_tx.send(()).unwrap();
    done_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the Tokio-touching cleanup completed, not panicked");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while ordinary_fiber_count(&ctx) != 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the fiber was unlinked framework-side"
        );
        std::thread::yield_now();
    }
}
