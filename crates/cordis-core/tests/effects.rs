//! Effect contract: one cleanup obligation per registration, owned by the
//! registering fiber's current generation, claimed exactly once by manual
//! control or the sequential LIFO drain (EF-01…EF-05, EF-07, EF-09).
//!
//! Covered here: Loading/Active admission against Pending/Failed refusal,
//! dispose/disarm arbitration against the drain, cancellation before and
//! after a winning dispose claim, cross-resource LIFO positions, attempt-all
//! drains whose failures normalize to `EffectFailure` without replacing a
//! primary lifecycle error, refusal rollback ownership, and outside-lock
//! cleanup destruction on every path. The tail section covers the one
//! task-specific composition, `Context::run` (LF-14/EF-08): commit-before-
//! start registration, refusals that start nothing, task-owned cleanup
//! draining before the join, panic containment at the join, caller
//! cancellation after the commit, and off-lock polling/output destruction.

mod common;

use common::{bounded, ordinary_fiber_count};
use cordis_core::effect::{
    EffectFailureKind, EffectRegistration, EffectRegistrationError, TaskRegistrationError,
};
use cordis_core::event::observer_sync;
use cordis_core::lifecycle::SpawnError;
use cordis_core::logger::BufferExporter;
use cordis_core::{
    BoxError, Context, Event, FiberHandle, FiberState, InjectSpec, Level, Plugin, PreparedPlugin,
    Routing, Service,
};
use parking_lot::Mutex;
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::task::Poll;
use std::time::Duration;

/// The one concrete apply-failure carrier the laboratory needs — `BoxError`
/// is deliberately not itself an `Error`.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct ApplyFailure(String);

/// A plugin whose apply body is the closure it carries — the effect
/// laboratory: registrations land on a real fiber's Loading generation.
/// The closure is `Fn` because a restart re-applies.
struct Effectful<F>(F);

impl<F> Plugin for Effectful<F>
where
    F: Fn(Context) -> Result<(), BoxError> + Send + 'static,
{
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = ApplyFailure;

    fn prepare(&self, _config: ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn apply(
        &self,
        ctx: Context,
        _prepared: &(),
    ) -> impl Future<Output = Result<(), ApplyFailure>> + Send {
        let outcome = (self.0)(ctx).map_err(|e| ApplyFailure(e.to_string()));
        async move { outcome }
    }
}

/// Spawn a plugin and wait out its initial settle.
async fn spawn_settled<P>(ctx: &Context, plugin: P) -> FiberHandle
where
    P: Plugin<Config = (), Input = (), ApplyError = ApplyFailure>,
{
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(plugin, ()))
        .await
        .expect("spawn admitted");
    let _ = fiber_handle.ready().await;
    fiber_handle
}

/// Stash-slot for a registration escaping its apply.
type RegistrationSlot = Arc<Mutex<Option<EffectRegistration>>>;

/// A one-shot barrier that can live behind an `Fn` apply: the take happens
/// through the mutex, so the apply closure stays reusable.
type BarrierSlot<T> = Arc<Mutex<Option<T>>>;

/// Drive one poll of a future whose pending state the test then abandons.
fn poll_once<F: Future>(fut: Pin<&mut F>) -> Poll<F::Output> {
    let mut task_cx = std::task::Context::from_waker(std::task::Waker::noop());
    fut.poll(&mut task_cx)
}

// ---------------------------------------------------------------------------
// EF-02: manual dispose runs the cleanup exactly once.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sync_cleanup_disposes_via_registration() {
    let ctx = Context::new();
    let ran = Arc::new(AtomicU32::new(0));
    let r = ran.clone();

    let registration = ctx
        .effect_sync(move || {
            r.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();

    assert_eq!(ran.load(Ordering::SeqCst), 0, "cleanup deferred");
    assert!(
        registration.dispose().await.expect("cleanup succeeded"),
        "the manual claim won"
    );
    assert_eq!(ran.load(Ordering::SeqCst), 1, "dispose ran the cleanup");
}

#[tokio::test]
async fn async_cleanup_disposes_via_registration() {
    let ctx = Context::new();
    let ran = Arc::new(AtomicU32::new(0));
    let r = ran.clone();

    let registration = ctx
        .effect(move || {
            let r = r.clone();
            async move {
                tokio::task::yield_now().await;
                r.fetch_add(1, Ordering::SeqCst);
            }
        })
        .unwrap();

    assert!(registration.dispose().await.expect("cleanup succeeded"));
    assert_eq!(
        ran.load(Ordering::SeqCst),
        1,
        "dispose awaited the async cleanup"
    );
}

// ---------------------------------------------------------------------------
// EF-02/EF-04: a winning dispose returns the exact EffectFailure, and the
// failed occurrence is permanently consumed — the drain never repeats it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispose_reports_returned_error_and_consumes_the_occurrence() {
    #[derive(Debug, thiserror::Error)]
    #[error("offline boom")]
    struct Offline;

    let ran = Arc::new(AtomicU32::new(0));
    let slot: RegistrationSlot = Arc::new(Mutex::new(None));
    let (s, r) = (slot.clone(), ran.clone());
    let fiber_handle = spawn_settled(
        &Context::new(),
        Effectful(move |ctx: Context| {
            *s.lock() = Some(
                ctx.effect_sync({
                    let r = r.clone();
                    move || -> Result<(), Offline> {
                        r.fetch_add(1, Ordering::SeqCst);
                        Err(Offline)
                    }
                })
                .unwrap(),
            );
            Ok(())
        }),
    )
    .await;
    let registration = slot.lock().take().unwrap();
    let failure = registration
        .dispose()
        .await
        .expect_err("the cleanup returned an error");
    assert_eq!(failure.kind(), EffectFailureKind::ReturnedError);
    assert_eq!(failure.diagnostic(), "offline boom");
    assert_eq!(ran.load(Ordering::SeqCst), 1, "cleanup ran once");

    fiber_handle.dispose().await.unwrap();
    assert_eq!(
        ran.load(Ordering::SeqCst),
        1,
        "a failed occurrence is consumed, not restored: the drain skips it"
    );
}

#[tokio::test]
async fn dispose_reports_panic_and_consumes_the_occurrence() {
    let ran = Arc::new(AtomicU32::new(0));
    let slot: RegistrationSlot = Arc::new(Mutex::new(None));
    let (s, r) = (slot.clone(), ran.clone());
    let fiber_handle = spawn_settled(
        &Context::new(),
        Effectful(move |ctx: Context| {
            *s.lock() = Some(
                ctx.effect_sync({
                    let r = r.clone();
                    move || -> () {
                        r.fetch_add(1, Ordering::SeqCst);
                        std::panic::panic_any("dispose panic payload".to_owned());
                    }
                })
                .unwrap(),
            );
            Ok(())
        }),
    )
    .await;

    let registration = slot.lock().take().unwrap();
    let failure = registration
        .dispose()
        .await
        .expect_err("the cleanup panicked");
    assert_eq!(failure.kind(), EffectFailureKind::Panic);
    assert_eq!(failure.diagnostic(), "dispose panic payload");

    fiber_handle.dispose().await.unwrap();
    assert_eq!(
        ran.load(Ordering::SeqCst),
        1,
        "the drain skips the consumed occurrence"
    );
}

// ---------------------------------------------------------------------------
// EF-03: disarm releases without running; stale control reports false.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disarm_releases_without_running() {
    let ran = Arc::new(AtomicU32::new(0));
    let slot: RegistrationSlot = Arc::new(Mutex::new(None));
    let (s, r) = (slot.clone(), ran.clone());
    let fiber_handle = spawn_settled(
        &Context::new(),
        Effectful(move |ctx: Context| {
            *s.lock() = Some(
                ctx.effect_sync({
                    let r = r.clone();
                    move || {
                        r.fetch_add(1, Ordering::SeqCst);
                    }
                })
                .unwrap(),
            );
            Ok(())
        }),
    )
    .await;

    assert!(
        slot.lock().take().unwrap().disarm(),
        "first claim wins the occurrence"
    );
    fiber_handle.dispose().await.unwrap();
    assert_eq!(
        ran.load(Ordering::SeqCst),
        0,
        "disarm must not run the cleanup"
    );
}

#[tokio::test]
async fn stale_control_reports_false_after_the_drain_won() {
    let ran = Arc::new(AtomicU32::new(0));
    let disarm_slot: RegistrationSlot = Arc::new(Mutex::new(None));
    let dispose_slot: RegistrationSlot = Arc::new(Mutex::new(None));
    let (ds, ps, r) = (disarm_slot.clone(), dispose_slot.clone(), ran.clone());
    let fiber_handle = spawn_settled(
        &Context::new(),
        Effectful(move |ctx: Context| {
            *ds.lock() = Some(
                ctx.effect_sync({
                    let r = r.clone();
                    move || {
                        r.fetch_add(1, Ordering::SeqCst);
                    }
                })
                .unwrap(),
            );
            *ps.lock() = Some(
                ctx.effect_sync({
                    let r = r.clone();
                    move || {
                        r.fetch_add(1, Ordering::SeqCst);
                    }
                })
                .unwrap(),
            );
            Ok(())
        }),
    )
    .await;

    fiber_handle.dispose().await.unwrap();
    assert_eq!(ran.load(Ordering::SeqCst), 2, "the drain claimed both");
    assert!(
        !disarm_slot.lock().take().unwrap().disarm(),
        "a drained occurrence disarms false"
    );
    let registration = dispose_slot.lock().take().unwrap();
    assert!(
        !registration
            .dispose()
            .await
            .expect("losing the claim is not a cleanup failure"),
        "a drained occurrence disposes Ok(false)"
    );
    assert_eq!(ran.load(Ordering::SeqCst), 2, "nothing ran twice");
}

// ---------------------------------------------------------------------------
// EF-03/EF-07: dropping the registration is inert; the committed occurrence
// stays generation-owned. A refused registration leaves the prepared
// resource under caller ownership for rollback.
// ---------------------------------------------------------------------------

/// A prepared external resource — the EF-07 sentinel for
/// rollback-versus-commit ownership. Tests observe it through a `Weak` so
/// the assertions measure exactly the framework's references.
struct PreparedResource;

#[tokio::test]
async fn commit_transfers_the_obligation_to_the_generation() {
    let cleaned = Arc::new(AtomicU32::new(0));
    let cell: BarrierSlot<Arc<PreparedResource>> = Arc::new(Mutex::new(None));
    let resource = Arc::new(PreparedResource);
    *cell.lock() = Some(resource.clone());
    let weak = Arc::downgrade(&resource);

    let (c, slot) = (cleaned.clone(), cell.clone());
    let fiber_handle = spawn_settled(
        &Context::new(),
        Effectful(move |ctx: Context| {
            let resource = slot.lock().take().expect("applied once");
            // dropping the returned registration immediately: inert —
            // ownership stays with the generation
            drop(ctx.effect_sync({
                let c = c.clone();
                move || {
                    c.fetch_add(1, Ordering::SeqCst);
                    drop(resource);
                }
            })?);
            Ok(())
        }),
    )
    .await;

    drop(resource);
    assert_eq!(cleaned.load(Ordering::SeqCst), 0, "nothing ran yet");
    assert_eq!(
        weak.strong_count(),
        1,
        "the committed cleanup obligation keeps the prepared resource alive"
    );

    fiber_handle.dispose().await.unwrap();
    assert_eq!(
        cleaned.load(Ordering::SeqCst),
        1,
        "the generation drain ran it"
    );
    assert!(
        weak.upgrade().is_none(),
        "the obligation's last reference was released with the drain"
    );
}

#[tokio::test]
async fn refusal_leaves_the_prepared_resource_with_the_caller() {
    let slot: Arc<Mutex<Option<Context>>> = Arc::new(Mutex::new(None));
    let s = slot.clone();
    let fiber_handle = spawn_settled(
        &Context::new(),
        Effectful(move |ctx: Context| {
            *s.lock() = Some(ctx);
            Ok(())
        }),
    )
    .await;
    fiber_handle.dispose().await.unwrap();
    let dead_ctx = slot.lock().take().unwrap();

    let cleaned = Arc::new(AtomicU32::new(0));
    let resource = Arc::new(PreparedResource);
    let weak = Arc::downgrade(&resource);
    let result = dead_ctx.effect_sync({
        let (c, resource) = (cleaned.clone(), resource.clone());
        move || {
            c.fetch_add(1, Ordering::SeqCst);
            drop(resource);
        }
    });
    assert!(
        matches!(result, Err(EffectRegistrationError::InactiveContext)),
        "a closed generation refuses"
    );
    assert_eq!(cleaned.load(Ordering::SeqCst), 0, "nothing ran");
    assert_eq!(
        weak.strong_count(),
        1,
        "the caller still holds the prepared resource — rollback is theirs"
    );

    // the refused closure did not linger anywhere: dropping the caller's
    // handle is the resource's last reference
    drop(resource);
    assert!(
        weak.upgrade().is_none(),
        "the refused closure was already released"
    );
}

// ---------------------------------------------------------------------------
// EF-01: admission — Loading and Active admit; stable Pending and Failed
// refuse. Every drain (restart, failed-apply rollback, disposal) is a
// sequential reverse-commit claim of the generation's obligations.
// ---------------------------------------------------------------------------

/// A service for the dependency-driven Pending probe.
struct Counter;

impl Service for Counter {
    const NAME: &'static str = "counter";
}

/// Provider of [`Counter`]; disposing its FiberHandle withdraws the service.
struct ProvidesCounter;

impl Plugin for ProvidesCounter {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = ApplyFailure;

    fn prepare(&self, _config: ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn apply(
        &self,
        ctx: Context,
        _prepared: &(),
    ) -> impl Future<Output = Result<(), ApplyFailure>> + Send {
        let outcome = ctx
            .provide(Arc::new(Counter))
            .map(|_| ())
            .map_err(|e| ApplyFailure(e.to_string()));
        async move { outcome }
    }
}

/// Dependent on [`Counter`]; stashes its context so the test can probe
/// admissions after the fiber converges back to Pending.
struct Dependent(Arc<Mutex<Option<Context>>>);

impl Plugin for Dependent {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = ApplyFailure;

    fn inject(&self) -> InjectSpec {
        InjectSpec::none().require(Counter::NAME)
    }

    fn prepare(&self, _config: ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn apply(
        &self,
        ctx: Context,
        _prepared: &(),
    ) -> impl Future<Output = Result<(), ApplyFailure>> + Send {
        *self.0.lock() = Some(ctx);
        std::future::ready(Ok(()))
    }
}

#[tokio::test]
async fn loading_and_active_generations_admit_cleanup() {
    let order: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let slot: Arc<Mutex<Option<Context>>> = Arc::new(Mutex::new(None));
    let (o, s) = (order.clone(), slot.clone());
    let fiber_handle = spawn_settled(
        &Context::new(),
        Effectful(move |ctx: Context| {
            // Loading admission: registration while apply runs
            ctx.effect_sync({
                let o = o.clone();
                move || o.lock().push("loading")
            })?;
            *s.lock() = Some(ctx);
            Ok(())
        }),
    )
    .await;

    // Active admission: the applied fiber's context still registers
    let ctx = slot.lock().clone().unwrap();
    ctx.effect_sync({
        let o = order.clone();
        move || o.lock().push("active")
    })
    .expect("an Active generation admits cleanup");

    fiber_handle.dispose().await.unwrap();
    assert_eq!(
        *order.lock(),
        vec!["active", "loading"],
        "both admitted, drained LIFO"
    );
}

#[tokio::test]
async fn pending_fiber_refuses_new_cleanup() {
    let ctx = Context::new();
    let provider = spawn_settled(&ctx, ProvidesCounter).await;

    let slot: Arc<Mutex<Option<Context>>> = Arc::new(Mutex::new(None));
    let dependent = spawn_settled(&ctx, Dependent(slot.clone())).await;

    // withdraw the dependency: the dependent unloads and parks Pending
    provider.dispose().await.unwrap();
    dependent
        .wait_state(FiberState::Pending, Duration::from_secs(5))
        .await
        .expect("the dependent settles Pending");

    let pending_ctx = slot.lock().clone().unwrap();
    assert!(
        matches!(
            pending_ctx.effect_sync(|| {}),
            Err(EffectRegistrationError::InactiveContext)
        ),
        "a stable Pending fiber rejects new generation-owned cleanup"
    );
    assert!(
        matches!(
            pending_ctx.effect(|| std::future::ready(())),
            Err(EffectRegistrationError::InactiveContext)
        ),
        "the async path shares the gate"
    );
}

#[tokio::test]
async fn failed_fiber_refuses_new_cleanup() {
    // a stable Failed fiber exists only past creation — an initial apply
    // failure leaves no resident Fiber (LF-01) — so the fixture parks
    // Failed through a restart: first apply healthy, the re-apply fails
    let slot: Arc<Mutex<Option<Context>>> = Arc::new(Mutex::new(None));
    let applies = Arc::new(AtomicU32::new(0));
    let (s, a) = (slot.clone(), applies.clone());
    let fiber_handle = spawn_settled(
        &Context::new(),
        Effectful(move |ctx: Context| {
            *s.lock() = Some(ctx);
            if a.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(())
            } else {
                Err("apply boom".into())
            }
        }),
    )
    .await;
    fiber_handle
        .restart()
        .await
        .expect_err("the re-apply failed");
    assert_eq!(
        fiber_handle.state(),
        FiberState::Failed,
        "the failed restart parks Failed"
    );

    let failed_ctx = slot.lock().clone().unwrap();
    assert!(
        matches!(
            failed_ctx.effect_sync(|| {}),
            Err(EffectRegistrationError::InactiveContext)
        ),
        "a stable Failed fiber rejects new generation-owned cleanup"
    );
}

#[tokio::test]
async fn effect_cleanup_runs_on_unload_and_on_dispose() {
    // restart drains the old generation and applies a fresh one; dispose
    // drains that — each generation's obligation runs exactly once
    let ran = Arc::new(AtomicU32::new(0));
    let r = ran.clone();
    let fiber_handle = spawn_settled(
        &Context::new(),
        Effectful(move |ctx: Context| {
            ctx.effect_sync({
                let r = r.clone();
                move || {
                    r.fetch_add(1, Ordering::SeqCst);
                }
            })?;
            Ok(())
        }),
    )
    .await;

    fiber_handle.restart().await.unwrap();
    assert_eq!(
        ran.load(Ordering::SeqCst),
        1,
        "restart drained the old generation"
    );

    fiber_handle.dispose().await.unwrap();
    assert_eq!(
        ran.load(Ordering::SeqCst),
        2,
        "dispose drained the new generation"
    );
}

#[tokio::test]
async fn effect_sync_holds_lifo_position_against_async_effects() {
    // sync and async cleanups share one LIFO order: later-committed
    // async cleanup tears down before the earlier sync one
    let order: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));

    let o = order.clone();
    let fiber_handle = spawn_settled(
        &Context::new(),
        Effectful(move |ctx: Context| {
            ctx.effect_sync({
                let o = o.clone();
                move || o.lock().push("sync-first")
            })?;
            ctx.effect({
                let o = o.clone();
                move || async move {
                    o.lock().push("async-second");
                }
            })?;
            Ok(())
        }),
    )
    .await;

    fiber_handle.dispose().await.unwrap();
    assert_eq!(*order.lock(), vec!["async-second", "sync-first"]);
}

// ---------------------------------------------------------------------------
// EF-01: cross-resource LIFO positions — one reverse-commit order across
// pure cleanups, listener unregistration, and exporter detachment.
// ---------------------------------------------------------------------------

struct Ping;

impl Event for Ping {
    const NAME: &'static str = "ping";
    type Args = ();
    type Output = ();
}

#[tokio::test]
async fn cross_resource_cleanup_holds_reverse_commit_positions() {
    let buffer = Arc::new(BufferExporter::new(16, Level::Debug).unwrap());
    let fired = Arc::new(AtomicUsize::new(0));

    let (b, f) = (buffer.clone(), fired.clone());
    let fiber_handle = spawn_settled(
        &Context::new(),
        Effectful(move |ctx: Context| {
            // commit order: tail, exporter, probe-a, listener, probe-b, top
            let tail_ctx = ctx.clone();
            ctx.effect(move || async move {
                tail_ctx.logger().warn("tail");
            })?;
            ctx.add_exporter(b.clone())?;
            let probe_a = ctx.clone();
            ctx.effect(move || async move {
                probe_a.logger().warn("a");
                probe_a
                    .emit::<Ping>(Routing::Scoped(probe_a.scope()), ())
                    .await
                    .unwrap();
            })?;
            let _listener = ctx
                .on::<Ping, _>(observer_sync({
                    let f = f.clone();
                    move |_, ()| -> Result<(), Infallible> {
                        f.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    }
                }))
                .unwrap();
            let probe_b = ctx.clone();
            ctx.effect(move || async move {
                probe_b.logger().warn("b");
                probe_b
                    .emit::<Ping>(Routing::Scoped(probe_b.scope()), ())
                    .await
                    .unwrap();
            })?;
            let top_ctx = ctx.clone();
            ctx.effect(move || async move {
                top_ctx.logger().warn("top");
            })?;
            Ok(())
        }),
    )
    .await;

    fiber_handle.dispose().await.unwrap();

    let texts: Vec<String> = buffer
        .snapshot()
        .into_iter()
        .map(|record| record.text().to_owned())
        .collect();
    assert_eq!(
        texts,
        ["top", "b", "a"],
        "strict reverse-commit drain: the exporter detaches after probe-a \
         but before tail, so only top/b/a are recorded"
    );
    assert_eq!(
        fired.load(Ordering::SeqCst),
        1,
        "the listener unregisters between probe-b and probe-a: only b's emit reaches it"
    );
}

// ---------------------------------------------------------------------------
// EF-04: drain failures are per-item, normalized, reported, and never
// replace the lifecycle operation's own outcome.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn failing_and_panicking_cleanups_do_not_block_the_drain() {
    // drain order: good-2, failing (reported), panicky (reported), good-1
    let order: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let buffer = Arc::new(BufferExporter::new(16, Level::Debug).unwrap());

    #[derive(Debug, thiserror::Error)]
    #[error("rollback boom")]
    struct Rollback;

    let o = order.clone();
    let ctx = Context::new();
    ctx.add_exporter(buffer.clone()).unwrap();
    let fiber_handle = spawn_settled(
        &ctx,
        Effectful(move |ctx: Context| {
            ctx.effect_sync({
                let o = o.clone();
                move || o.lock().push("good-1")
            })?;
            ctx.effect_sync(|| -> Result<(), Rollback> { Err(Rollback) })?;
            ctx.effect_sync(|| -> () { panic!("cleanup exploded") })?;
            ctx.effect_sync({
                let o = o.clone();
                move || o.lock().push("good-2")
            })?;
            Ok(())
        }),
    )
    .await;

    fiber_handle
        .dispose()
        .await
        .expect("cleanup failures never fail the disposal itself");
    assert_eq!(
        *order.lock(),
        vec!["good-2", "good-1"],
        "attempt-all: every remaining cleanup ran despite two failures"
    );
    assert_eq!(fiber_handle.state(), FiberState::Disposed);

    let texts: Vec<String> = buffer
        .snapshot()
        .into_iter()
        .map(|record| record.text().to_owned())
        .collect();
    assert!(
        texts
            .iter()
            .any(|t| t == "cordis: effect cleanup returned an error: rollback boom"),
        "the returned error was normalized and reported, got {texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t == "cordis: effect cleanup panicked: cleanup exploded"),
        "the panic was normalized and reported, got {texts:?}"
    );
}

#[tokio::test]
async fn rollback_cleanup_failure_never_replaces_the_apply_error() {
    let buffer = Arc::new(BufferExporter::new(16, Level::Debug).unwrap());
    let rolled_back = Arc::new(AtomicU32::new(0));

    #[derive(Debug, thiserror::Error)]
    #[error("secondary cleanup boom")]
    struct Secondary;

    let rb = rolled_back.clone();
    let ctx = Context::new();
    ctx.add_exporter(buffer.clone()).unwrap();
    let outcome = ctx
        .spawn(PreparedPlugin::from_input(
            Effectful(move |ctx: Context| {
                ctx.effect_sync({
                    let rb = rb.clone();
                    move || {
                        rb.fetch_add(1, Ordering::SeqCst);
                    }
                })?;
                ctx.effect_sync(|| -> Result<(), Secondary> { Err(Secondary) })?;
                Err("primary apply boom".into())
            }),
            (),
        ))
        .await;

    // LF-01/LF-07: the initial apply failure IS the spawn's answer —
    // the primary failure, normalized once, never replaced by a
    // secondary rollback-cleanup failure
    let SpawnError::InitialApply(failure) = outcome.expect_err("the initial apply failed") else {
        panic!("expected InitialApply");
    };
    assert!(
        failure.diagnostic().contains("primary apply boom"),
        "the lifecycle failure stays the apply failure: {failure}"
    );
    assert!(
        !failure.diagnostic().contains("secondary cleanup boom"),
        "a rollback cleanup failure never replaces it: {failure}"
    );
    assert_eq!(
        rolled_back.load(Ordering::SeqCst),
        1,
        "rollback was attempt-all"
    );
    assert_eq!(
        ordinary_fiber_count(&ctx),
        0,
        "no resident attempted Fiber survives the failed creation"
    );

    let texts: Vec<String> = buffer
        .snapshot()
        .into_iter()
        .map(|record| record.text().to_owned())
        .collect();
    assert!(
        texts
            .iter()
            .any(|t| t == "cordis: effect cleanup returned an error: secondary cleanup boom"),
        "the rollback failure was reported as a diagnostic: {texts:?}"
    );
}

// ---------------------------------------------------------------------------
// EF-02: a winning dispose completes under framework ownership, independent
// of caller polling; cancellation before the claim changes nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn winning_dispose_completes_after_caller_cancellation() {
    let ctx = Context::new();
    let started = Arc::new(AtomicBool::new(false));
    let completed = Arc::new(AtomicBool::new(false));
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();

    let (s, c) = (started.clone(), completed.clone());
    let registration = ctx
        .effect(move || {
            let (s, c) = (s.clone(), c.clone());
            async move {
                s.store(true, Ordering::SeqCst);
                let _ = release_rx.await;
                c.store(true, Ordering::SeqCst);
            }
        })
        .unwrap();

    // one poll: the claim is won and execution detaches; the blocked
    // cleanup keeps the dispose future pending
    let mut fut = Box::pin(registration.dispose());
    assert!(
        poll_once(fut.as_mut()).is_pending(),
        "the cleanup is blocked, so the wait is pending"
    );
    drop(fut);

    // cancellation after the claim abandons only the wait
    bounded(5000, async {
        while !started.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the framework-owned cleanup started");
    release_tx.send(()).unwrap();
    bounded(5000, async {
        while !completed.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the cleanup completed with nobody polling the dispose");
}

#[tokio::test]
async fn dispose_cancelled_before_the_claim_changes_nothing() {
    let ran = Arc::new(AtomicU32::new(0));
    let slot: RegistrationSlot = Arc::new(Mutex::new(None));
    let (s, r) = (slot.clone(), ran.clone());
    let fiber_handle = spawn_settled(
        &Context::new(),
        Effectful(move |ctx: Context| {
            *s.lock() = Some(
                ctx.effect_sync({
                    let r = r.clone();
                    move || {
                        r.fetch_add(1, Ordering::SeqCst);
                    }
                })
                .unwrap(),
            );
            Ok(())
        }),
    )
    .await;

    // never even polled: cancellation before the claim — the occurrence
    // stays generation-owned
    drop(slot.lock().take().unwrap().dispose());

    fiber_handle.dispose().await.unwrap();
    assert_eq!(
        ran.load(Ordering::SeqCst),
        1,
        "the generation drain still owns and runs the occurrence"
    );
}

#[tokio::test]
async fn winning_dispose_beats_an_in_flight_drain() {
    let ran = Arc::new(AtomicU32::new(0));
    let slot: RegistrationSlot = Arc::new(Mutex::new(None));
    let started: BarrierSlot<tokio::sync::oneshot::Sender<()>> = Arc::new(Mutex::new(None));
    let release: BarrierSlot<tokio::sync::oneshot::Receiver<()>> = Arc::new(Mutex::new(None));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    *started.lock() = Some(started_tx);
    *release.lock() = Some(release_rx);

    let (s, r) = (slot.clone(), ran.clone());
    let fiber_handle = spawn_settled(
        &Context::new(),
        Effectful(move |ctx: Context| {
            // commit order: target, then blocker — the LIFO drain blocks
            // *before* claiming the target
            *s.lock() = Some(
                ctx.effect_sync({
                    let r = r.clone();
                    move || {
                        r.fetch_add(1, Ordering::SeqCst);
                    }
                })
                .unwrap(),
            );
            ctx.effect({
                let started = started.lock().take().expect("applied once");
                let release = release.lock().take().expect("applied once");
                move || async move {
                    let _ = started.send(());
                    let _ = release.await;
                }
            })?;
            Ok(())
        }),
    )
    .await;

    let drainer = tokio::spawn({
        let fiber_handle = fiber_handle.clone();
        async move { fiber_handle.dispose().await }
    });
    bounded(5000, started_rx)
        .await
        .expect("the drain is blocked inside the newer cleanup")
        .unwrap();

    let registration = slot.lock().take().unwrap();
    let claimed = registration
        .dispose()
        .await
        .expect("the manual cleanup succeeded");
    assert!(claimed, "the manual claim won the race");
    assert_eq!(
        ran.load(Ordering::SeqCst),
        1,
        "the manual dispose ran it once"
    );

    release_tx.send(()).unwrap();
    bounded(5000, drainer)
        .await
        .expect("the drain completed")
        .unwrap()
        .unwrap();
    assert_eq!(
        ran.load(Ordering::SeqCst),
        1,
        "the drain skipped the claimed occurrence"
    );
}

#[tokio::test]
async fn winning_disarm_beats_an_in_flight_drain() {
    let ran = Arc::new(AtomicU32::new(0));
    let slot: RegistrationSlot = Arc::new(Mutex::new(None));
    let started: BarrierSlot<tokio::sync::oneshot::Sender<()>> = Arc::new(Mutex::new(None));
    let release: BarrierSlot<tokio::sync::oneshot::Receiver<()>> = Arc::new(Mutex::new(None));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    *started.lock() = Some(started_tx);
    *release.lock() = Some(release_rx);

    let (s, r) = (slot.clone(), ran.clone());
    let fiber_handle = spawn_settled(
        &Context::new(),
        Effectful(move |ctx: Context| {
            // commit order: target, then blocker — the LIFO drain blocks
            // *before* claiming the target
            *s.lock() = Some(
                ctx.effect_sync({
                    let r = r.clone();
                    move || {
                        r.fetch_add(1, Ordering::SeqCst);
                    }
                })
                .unwrap(),
            );
            ctx.effect({
                let started = started.lock().take().expect("applied once");
                let release = release.lock().take().expect("applied once");
                move || async move {
                    let _ = started.send(());
                    let _ = release.await;
                }
            })?;
            Ok(())
        }),
    )
    .await;

    let drainer = tokio::spawn({
        let fiber_handle = fiber_handle.clone();
        async move { fiber_handle.dispose().await }
    });
    bounded(5000, started_rx)
        .await
        .expect("the drain is blocked inside the newer cleanup")
        .unwrap();

    let registration = slot.lock().take().unwrap();
    assert!(
        registration.disarm(),
        "the manual disarm won the claim mid-drain"
    );

    release_tx.send(()).unwrap();
    bounded(5000, drainer)
        .await
        .expect("the drain completed")
        .unwrap()
        .unwrap();
    assert_eq!(
        ran.load(Ordering::SeqCst),
        0,
        "the disarmed occurrence was released without running, and the drain skipped it"
    );
}

// ADR 0010 decision 5 — the gated-path race proper, re-homed from the
// retired List surface (ticket 56's evidence migration): effect
// registrations racing a dispose of the registering fiber must never
// strand a committed cleanup. The gated recheck refuses registrations
// that lose the race to the drain; a registration that wins is
// snapshotted by the drain and its cleanup runs. Either way, once the
// dispose completes, every accepted registration has been drained exactly
// once — a stranded obligation (the v1 zombie class) would survive here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn registration_racing_dispose_never_strands_a_cleanup() {
    for round in 0..64 {
        let root = Context::new();
        let ctx_slot: Arc<Mutex<Option<Context>>> = Arc::new(Mutex::new(None));
        let accepted = Arc::new(AtomicU32::new(0));
        let ran = Arc::new(AtomicU32::new(0));
        let fiber_handle = spawn_settled(&root, {
            let ctx_slot = ctx_slot.clone();
            Effectful(move |ctx: Context| {
                *ctx_slot.lock() = Some(ctx);
                Ok(())
            })
        })
        .await;
        let scoped = ctx_slot.lock().take().expect("the apply ran");

        let push_ctx = scoped.clone();
        let (push_accepted, push_ran) = (accepted.clone(), ran.clone());
        let registrations = tokio::spawn(async move {
            for i in 0..32 {
                // the yield pattern varies per round so different
                // interleavings of registration and the drain are exercised
                if (i + round) % 3 == 0 {
                    tokio::task::yield_now().await;
                }
                if push_ctx
                    .effect_sync({
                        let ran = push_ran.clone();
                        move || {
                            ran.fetch_add(1, Ordering::SeqCst);
                        }
                    })
                    .is_ok()
                {
                    push_accepted.fetch_add(1, Ordering::SeqCst);
                }
            }
        });

        // dispose concurrently with the registrations; the yield pattern
        // above lands it before, inside, and after the burst across rounds
        tokio::task::yield_now().await;
        fiber_handle.dispose().await.unwrap();
        registrations.await.unwrap();

        assert_eq!(
            accepted.load(Ordering::SeqCst),
            ran.load(Ordering::SeqCst),
            "round {round}: an accepted registration was never drained"
        );
    }
}

// ---------------------------------------------------------------------------
// EF-02, off-runtime arm: with no Tokio runtime current, the claim still
// detaches onto a framework thread — the cleanup completes even after the
// caller abandons the dispose future, and an attentive caller receives
// Ok(true). Exactly-once and the claim's boolean match the on-runtime path.
// ---------------------------------------------------------------------------

/// Minimal std block_on for the off-runtime arm — the contract tier
/// cannot use `futures` (not a dev-dependency), and the point is proving
/// the path works with zero Tokio involvement.
struct ThreadWaker(std::thread::Thread);

impl std::task::Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn block_on<F: Future>(fut: F) -> F::Output {
    let waker = std::task::Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut task_cx = std::task::Context::from_waker(&waker);
    let mut fut = Box::pin(fut);
    loop {
        if let Poll::Ready(out) = fut.as_mut().poll(&mut task_cx) {
            return out;
        }
        std::thread::park();
    }
}

#[test]
fn dispose_off_runtime_completes_framework_owned() {
    let ctx = Context::new();

    // abandoned after one poll: the claim is won, the wait dropped
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let registration = ctx
        .effect_sync(move || {
            started_tx.send(()).unwrap();
            // blocking the driving thread is safe: the fallback owns a
            // dedicated thread per off-runtime dispose
            release_rx.recv().unwrap();
            done_tx.send(()).unwrap();
        })
        .unwrap();

    std::thread::spawn(move || {
        let mut fut = Box::pin(registration.dispose());
        assert!(
            poll_once(fut.as_mut()).is_pending(),
            "claimed and blocked on the release"
        );
        // abandoned: the wait is dropped here, the cleanup continues
    })
    .join()
    .unwrap();

    started_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the cleanup started on the framework's thread");
    release_tx.send(()).unwrap();
    done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the cleanup completed after the caller abandoned");

    // an attentive caller on a plain thread receives the outcome
    let registration = ctx.effect_sync(|| {}).unwrap();
    let claimed = std::thread::spawn(move || block_on(registration.dispose()))
        .join()
        .unwrap()
        .expect("the cleanup succeeded");
    assert!(claimed, "the off-runtime claim won");
}

// ---------------------------------------------------------------------------
// EF-02/EF-04: a winning dispose whose caller abandoned the wait reports
// the cleanup's failure through the fiber's diagnostics — delivered exactly
// once: returned to a waiting caller, otherwise reported.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn abandoned_dispose_failure_is_reported() {
    #[derive(Debug, thiserror::Error)]
    #[error("abandoned boom")]
    struct Abandoned;

    let buffer = Arc::new(BufferExporter::new(16, Level::Debug).unwrap());
    let slot: RegistrationSlot = Arc::new(Mutex::new(None));
    let started: BarrierSlot<tokio::sync::oneshot::Sender<()>> = Arc::new(Mutex::new(None));
    let release: BarrierSlot<tokio::sync::oneshot::Receiver<()>> = Arc::new(Mutex::new(None));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    *started.lock() = Some(started_tx);
    *release.lock() = Some(release_rx);

    let (s, b) = (slot.clone(), buffer.clone());
    let fiber_handle = spawn_settled(
        &Context::new(),
        Effectful(move |ctx: Context| {
            ctx.add_exporter(b.clone())?;
            *s.lock() = Some(
                ctx.effect({
                    let started = started.lock().take().expect("applied once");
                    let release = release.lock().take().expect("applied once");
                    move || async move {
                        let _ = started.send(());
                        let _ = release.await;
                        Err(Abandoned)
                    }
                })
                .unwrap(),
            );
            Ok(())
        }),
    )
    .await;

    // win the claim, then abandon the wait before the cleanup unblocks
    let registration = slot.lock().take().unwrap();
    let mut fut = Box::pin(registration.dispose());
    assert!(poll_once(fut.as_mut()).is_pending());
    drop(fut);

    bounded(5000, started_rx)
        .await
        .expect("the framework-owned cleanup started")
        .unwrap();
    release_tx.send(()).unwrap();

    // the caller is gone, so the failure lands on the fiber's diagnostics
    bounded(5000, async {
        loop {
            let texts: Vec<String> = buffer
                .snapshot()
                .into_iter()
                .map(|record| record.text().to_owned())
                .collect();
            if texts
                .iter()
                .any(|t| t == "cordis: effect cleanup returned an error: abandoned boom")
            {
                return texts;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the abandoned failure was reported");

    fiber_handle.dispose().await.unwrap();
    let reports: Vec<String> = buffer
        .snapshot()
        .into_iter()
        .map(|record| record.text().to_owned())
        .filter(|t| t.contains("abandoned boom"))
        .collect();
    assert_eq!(reports.len(), 1, "reported exactly once: {reports:?}");
}

// ---------------------------------------------------------------------------
// EF-01: a registration attempted while a failed apply's rollback drain is
// in flight is refused — the closed gate cannot leak the obligation into
// the next generation.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn registration_during_failed_rollback_is_refused() {
    let slot: Arc<Mutex<Option<Context>>> = Arc::new(Mutex::new(None));
    let started: BarrierSlot<tokio::sync::oneshot::Sender<()>> = Arc::new(Mutex::new(None));
    let release: BarrierSlot<tokio::sync::oneshot::Receiver<()>> = Arc::new(Mutex::new(None));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    *started.lock() = Some(started_tx);
    *release.lock() = Some(release_rx);
    let rolled_back = Arc::new(AtomicU32::new(0));

    let (s, rb) = (slot.clone(), rolled_back.clone());
    let ctx = Context::new();
    let spawn = tokio::spawn({
        let ctx = ctx.clone();
        async move {
            ctx.spawn(PreparedPlugin::from_input(
                Effectful(move |ctx: Context| {
                    // rollback drain order: blocker first (LIFO), marker last
                    ctx.effect_sync({
                        let rb = rb.clone();
                        move || {
                            rb.fetch_add(1, Ordering::SeqCst);
                        }
                    })?;
                    ctx.effect({
                        let started = started.lock().take().expect("applied once");
                        let release = release.lock().take().expect("applied once");
                        move || async move {
                            let _ = started.send(());
                            let _ = release.await;
                        }
                    })?;
                    *s.lock() = Some(ctx);
                    Err("apply boom".into())
                }),
                (),
            ))
            .await
        }
    });

    bounded(5000, started_rx)
        .await
        .expect("the rollback drain is blocked mid-cleanup")
        .unwrap();

    let failed_ctx = slot.lock().clone().unwrap();
    assert!(
        matches!(
            failed_ctx.effect_sync(|| {}),
            Err(EffectRegistrationError::InactiveContext)
        ),
        "the Failed gate refuses mid-rollback — no leak into the next generation"
    );

    release_tx.send(()).unwrap();
    // LF-01: the failed creation returns the exact InitialApply failure
    // and leaves no resident attempted Fiber (the ticket-22 contract
    // this test's tail now rides)
    let err = bounded(5000, spawn)
        .await
        .expect("spawn completes after rollback")
        .unwrap()
        .expect_err("the failed initial apply refuses the FiberHandle");
    assert!(
        matches!(err, SpawnError::InitialApply(_)),
        "the apply failure is the creation's answer: {err:?}"
    );
    assert_eq!(
        rolled_back.load(Ordering::SeqCst),
        1,
        "the rollback drain finished attempt-all"
    );
    assert_eq!(
        ordinary_fiber_count(&ctx),
        0,
        "no resident attempted Fiber survives the failed creation"
    );
}

// ---------------------------------------------------------------------------
// EF-09: user-controlled destruction and result conversion run outside the
// cleanup journal's synchronization — on refusal, manual claim, and drain.
// ---------------------------------------------------------------------------

/// A capture whose Drop re-enters the fiber's registration seam.
///
/// Discrimination note: the re-entrant `effect_sync` only exercises the
/// journal mutex when the target fiber's generation gate passes the
/// pre-lock admission check — i.e. on a live (Loading/Active) fiber. The
/// disarm probe below re-enters its *live* fiber, so a drop under the
/// journal lock would deadlock the (non-reentrant) mutex and fire the
/// watchdog; the refusal and drain probes re-enter closed fibers, whose
/// gate refuses before the lock — they pin the visible refusal/no-hang
/// behavior on those paths while the structural drop-after-release
/// discipline is what the gated seam documents and the disarm/conversion
/// probes enforce against a live lock.
struct ReentrantOnDrop {
    ctx: Context,
}

impl Drop for ReentrantOnDrop {
    fn drop(&mut self) {
        let _ = self.ctx.effect_sync(|| {});
    }
}

#[tokio::test]
async fn refused_cleanup_drops_outside_synchronization() {
    let slot: Arc<Mutex<Option<Context>>> = Arc::new(Mutex::new(None));
    let s = slot.clone();
    let fiber_handle = spawn_settled(
        &Context::new(),
        Effectful(move |ctx: Context| {
            *s.lock() = Some(ctx);
            Ok(())
        }),
    )
    .await;
    // refusal path: the framework must release the refused cleanup
    // promptly (its Drop runs here) and without tripping any lock — the
    // re-entry itself refuses pre-lock on this dead fiber (see the
    // sentinel's discrimination note), so this pins refusal behavior while
    // the live-fiber probes carry the lock-position law
    fiber_handle.dispose().await.unwrap();
    let dead_ctx = slot.lock().take().unwrap();

    common::deadlock_watchdog(
        "a refused cleanup must drop without tripping synchronization",
        move || {
            let refused = {
                let probe = ReentrantOnDrop {
                    ctx: dead_ctx.clone(),
                };
                dead_ctx.effect_sync(move || drop(probe))
            };
            assert!(refused.is_err(), "the closed generation refuses");
        },
    );
}

#[tokio::test]
async fn disarmed_cleanup_drops_outside_synchronization() {
    let ctx = Context::new();
    let probe = ReentrantOnDrop { ctx: ctx.clone() };
    let registration = ctx.effect_sync(move || drop(probe)).unwrap();
    common::deadlock_watchdog(
        "a disarmed cleanup must drop outside the journal lock",
        move || {
            assert!(registration.disarm(), "the claim won");
        },
    );
}

#[tokio::test]
async fn drained_cleanup_drops_outside_synchronization() {
    let fiber_handle = spawn_settled(
        &Context::new(),
        Effectful(move |ctx: Context| {
            let probe = ReentrantOnDrop { ctx: ctx.clone() };
            ctx.effect_sync(move || drop(probe))?;
            Ok(())
        }),
    )
    .await;
    // the fiber is Unloading while the drain runs, so the sentinel's
    // re-entrant registration refuses before touching the journal lock —
    // this is the no-hang smoke for the drain path; the discriminating
    // drop-outside-the-lock evidence is the live-fiber disarm probe above
    bounded(5000, fiber_handle.dispose())
        .await
        .expect("the drain was not blocked by a cleanup Drop")
        .unwrap();
}

#[tokio::test]
async fn result_conversion_runs_outside_synchronization() {
    // the cleanup's error renders through user Display code at execution
    // time; that conversion re-enters registration and must not deadlock
    struct ReentrantError(Context);

    impl std::fmt::Display for ReentrantError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let _ = self.0.effect_sync(|| {});
            f.write_str("reentrant diagnostic")
        }
    }

    impl std::fmt::Debug for ReentrantError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("ReentrantError")
        }
    }

    impl std::error::Error for ReentrantError {}

    let ctx = Context::new();
    let probe = ctx.clone();
    let registration = ctx
        .effect_sync(move || -> Result<(), ReentrantError> { Err(ReentrantError(probe)) })
        .unwrap();

    let failure = bounded(5000, registration.dispose())
        .await
        .expect("conversion deadlocked under synchronization")
        .expect_err("the cleanup returned an error");
    assert_eq!(failure.kind(), EffectFailureKind::ReturnedError);
    assert_eq!(failure.diagnostic(), "reentrant diagnostic");
}

// ---------------------------------------------------------------------------
// LF-14/EF-08: `Context::run` — the one task-specific cleanup composition.
// Registration commits before the task starts; an inactive or
// executor-unavailable refusal starts nothing; the drain runs the task's
// own later cleanup first and joins last; a panicking task is contained
// and reported at that join; the task's output is consumed (no Send
// bound) and destroyed outside framework locks; a committed registration
// stays generation-owned no matter what the caller does afterwards.
// ---------------------------------------------------------------------------

/// Probe bundle for the drain choreography rows: the task's own cleanup
/// (`pump_effect_ran`), the task's completion (`task_finished`), a later
/// fiber-level cleanup (`after_effect_ran`), the synchronization that the
/// task-side registration has landed (`task_entered`), and the
/// cancellation row's liveness counter with its wakeup (`received` /
/// `received_notify`).
#[derive(Clone, Default)]
struct RunProbes {
    task_finished: Arc<AtomicU32>,
    pump_effect_ran: Arc<AtomicU32>,
    after_effect_ran: Arc<AtomicU32>,
    received: Arc<AtomicU32>,
    task_entered: Arc<tokio::sync::Notify>,
    received_notify: Arc<tokio::sync::Notify>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_drain_joins_after_the_tasks_own_effects_lifo() {
    let probes = RunProbes::default();
    let fiber_handle = spawn_settled(
        &Context::new(),
        Effectful({
            let probes = probes.clone();
            move |ctx: Context| {
                let (tx, mut rx) = tokio::sync::mpsc::channel::<u32>(8);
                let task_ctx = ctx.clone();
                let task_probes = probes.clone();
                // the fiber-bound task: registers its own cleanup (the
                // stream teardown), then pumps until the channel closes
                ctx.run(async move {
                    task_ctx
                        .effect_sync(move || {
                            task_probes.pump_effect_ran.fetch_add(1, Ordering::SeqCst);
                            drop(tx);
                        })
                        .expect("the task's own generation admits");
                    task_probes.task_entered.notify_one();
                    while rx.recv().await.is_some() {}
                    task_probes.task_finished.fetch_add(1, Ordering::SeqCst);
                })?;
                // a later cleanup on the fiber itself: LIFO drains it
                // before the join-effect
                let after = probes.after_effect_ran.clone();
                ctx.effect_sync(move || {
                    after.fetch_add(1, Ordering::SeqCst);
                })?;
                Ok(())
            }
        }),
    )
    .await;
    // the task's own cleanup is registered — the LIFO order under test
    // is fully assembled before the drain starts (a drain landing before
    // the task's first poll is the closed-generation refusal's contract,
    // not this one's)
    probes.task_entered.notified().await;

    // The drain order must be: every later cleanup (the task's stream
    // teardown, the fiber-level one) first, the join last. If the join
    // ran first this dispose would deadlock against a task whose stream
    // never ends — surfacing as the bounded guard's None, not a hang.
    bounded(2000, fiber_handle.dispose())
        .await
        .expect("drain must join the task after tearing down its cleanups")
        .unwrap();

    assert_eq!(
        probes.pump_effect_ran.load(Ordering::SeqCst),
        1,
        "the task's own cleanup ran at drain"
    );
    assert_eq!(
        probes.after_effect_ran.load(Ordering::SeqCst),
        1,
        "fiber cleanups registered after run still ran (LIFO before the join)"
    );
    assert_eq!(
        probes.task_finished.load(Ordering::SeqCst),
        1,
        "the task observed its stream ending and returned before the join"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_drain_join_contains_a_panicking_task() {
    let ctx = Context::new();
    // the report half of the containment contract is observable: the
    // contained panic rides the fiber's logger to the root's exporters
    // as one warn record (Warn filters out every non-report record)
    let reports = Arc::new(BufferExporter::new(8, Level::Warn).unwrap());
    ctx.add_exporter(reports.clone()).unwrap();
    let after = Arc::new(AtomicU32::new(0));

    let fiber_handle = spawn_settled(
        &ctx,
        Effectful({
            let after = after.clone();
            move |ctx: Context| {
                ctx.run(async {
                    panic!("task exploded at drain");
                })?;
                let after = after.clone();
                ctx.effect_sync(move || {
                    after.fetch_add(1, Ordering::SeqCst);
                })?;
                Ok(())
            }
        }),
    )
    .await;

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    // the task panics at its first poll; the drain-join recovers the
    // payload from the JoinHandle inside the containment boundary — the
    // dispose completes, unwinds no further, the panic is reported to
    // the fiber's exporters, and the remaining LIFO cleanups still run
    let outcome = bounded(2000, fiber_handle.dispose()).await;
    std::panic::set_hook(default_hook);
    outcome
        .expect("drain-join contains the panicking task")
        .unwrap();
    assert_eq!(
        after.load(Ordering::SeqCst),
        1,
        "the drain continued past the contained task panic"
    );
    let texts: Vec<String> = reports
        .snapshot()
        .into_iter()
        .map(|record| record.text().to_owned())
        .collect();
    assert_eq!(
        texts,
        ["cordis: ctx.run task panicked: task exploded at drain".to_owned()],
        "the contained task panic is reported through the fiber's logger"
    );
}

#[tokio::test]
async fn run_off_the_runtime_refuses_and_starts_nothing() {
    let slot: Arc<Mutex<Option<Context>>> = Arc::new(Mutex::new(None));
    let fiber_handle = spawn_settled(
        &Context::new(),
        Effectful({
            let slot = slot.clone();
            move |ctx: Context| {
                *slot.lock() = Some(ctx);
                Ok(())
            }
        }),
    )
    .await;
    let probe = slot.lock().clone().unwrap();
    let started = Arc::new(AtomicU32::new(0));
    let s = started.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // plain thread: no tokio handle current
        let err = probe
            .run(async move {
                s.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap_err();
        tx.send(err).unwrap();
    })
    .join()
    .unwrap();
    assert_eq!(
        rx.recv().unwrap(),
        TaskRegistrationError::ExecutorUnavailable
    );
    assert_eq!(
        started.load(Ordering::SeqCst),
        0,
        "an executor-unavailable refusal never polled the task"
    );
    // and nothing was committed either: a phantom join-effect would
    // strand the drain waiting on a spawn that never comes
    bounded(2000, fiber_handle.dispose())
        .await
        .expect("the refusal left nothing generation-owned behind")
        .unwrap();
}

#[tokio::test]
async fn run_on_a_disposed_fiber_refuses_and_starts_nothing() {
    let slot: Arc<Mutex<Option<Context>>> = Arc::new(Mutex::new(None));
    let fiber_handle = spawn_settled(
        &Context::new(),
        Effectful({
            let slot = slot.clone();
            move |ctx: Context| {
                *slot.lock() = Some(ctx);
                Ok(())
            }
        }),
    )
    .await;
    fiber_handle.dispose().await.unwrap();

    let dead = slot.lock().clone().unwrap();
    let started = Arc::new(AtomicU32::new(0));
    let s = started.clone();
    let err = dead
        .run(async move {
            s.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap_err();
    assert_eq!(err, TaskRegistrationError::InactiveContext);
    assert_eq!(
        started.load(Ordering::SeqCst),
        0,
        "an inactive refusal never polled the task"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_task_stays_generation_owned_after_caller_cancellation() {
    // `run` is synchronous: cancellation of the calling future can only
    // ever arrive *after* the registration commit it cannot undo.
    let slot: Arc<Mutex<Option<Context>>> = Arc::new(Mutex::new(None));
    let fiber_handle = spawn_settled(
        &Context::new(),
        Effectful({
            let slot = slot.clone();
            move |ctx: Context| {
                *slot.lock() = Some(ctx);
                Ok(())
            }
        }),
    )
    .await;
    let fiber_ctx = slot.lock().clone().unwrap();

    let probes = RunProbes::default();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<u32>(8);
    // the caller registers the pump, then pends forever; the abort lands
    // strictly after `run` returned Ok — after the commit
    let caller = tokio::spawn({
        let fiber_ctx = fiber_ctx.clone();
        let probes = probes.clone();
        let tx_in_task = tx.clone();
        async move {
            let task_ctx = fiber_ctx.clone();
            let task_probes = probes.clone();
            fiber_ctx
                .run(async move {
                    task_ctx
                        .effect_sync(move || {
                            task_probes.pump_effect_ran.fetch_add(1, Ordering::SeqCst);
                            drop(tx_in_task);
                        })
                        .expect("the task's own generation admits");
                    task_probes.task_entered.notify_one();
                    while rx.recv().await.is_some() {
                        task_probes.received.fetch_add(1, Ordering::SeqCst);
                        task_probes.received_notify.notify_one();
                    }
                    task_probes.task_finished.fetch_add(1, Ordering::SeqCst);
                })
                .expect("registration commits before the caller can be cancelled");
            std::future::pending::<()>().await
        }
    });
    probes.task_entered.notified().await;
    caller.abort();
    assert!(
        caller.await.unwrap_err().is_cancelled(),
        "the caller was cancelled after the commit"
    );

    // the cancelled caller changed nothing: the task still pumps
    tx.send(7).await.unwrap();
    bounded(2000, {
        let probes = probes.clone();
        async move {
            while probes.received.load(Ordering::SeqCst) == 0 {
                probes.received_notify.notified().await;
            }
        }
    })
    .await
    .expect("the task outlived its cancelled caller");

    // and the drain still runs the task's own cleanup first, then joins
    drop(tx);
    bounded(2000, fiber_handle.dispose())
        .await
        .expect("drain joins the abandoned task")
        .unwrap();
    assert_eq!(
        probes.pump_effect_ran.load(Ordering::SeqCst),
        1,
        "the task's own cleanup ran at drain"
    );
    assert_eq!(
        probes.task_finished.load(Ordering::SeqCst),
        1,
        "the task observed its stream ending and returned before the join"
    );
}

#[tokio::test]
async fn run_task_polling_and_output_destruction_stay_outside_framework_locks() {
    /// The task's output: a value whose Drop re-enters the journal. If
    /// the framework destroyed the output under the disposables lock,
    /// this re-entry would deadlock.
    ///
    /// Discrimination note (which framework locks this actually pins):
    /// the re-entrant `effect_sync` targets the live root fiber, so it
    /// exercises the journal mutex — a poll or destruction under that
    /// lock deadlocks the non-reentrant mutex and fires the watchdog.
    /// The `RunSlot` cell lock is NOT reachable from this probe, and no
    /// public probe can reach it: the cell is not generic over the output,
    /// so the output cannot be there to drop. A mutant that moved the
    /// output into the cell (retention) would force it across the
    /// executor's spawn boundary and impose `Send` — pinned failing by
    /// `ui-effect/pass/run_task_output_needs_no_send.rs`. The settle-ctx
    /// attribution is a task-local `RefCell`, not a mutex.
    struct DropRegisters {
        ctx: Context,
        done: Arc<tokio::sync::Notify>,
    }
    impl Drop for DropRegisters {
        fn drop(&mut self) {
            let _ = self.ctx.effect_sync(|| {}).expect("the root admits");
            self.done.notify_one();
        }
    }

    let ctx = Context::new();
    let done = Arc::new(tokio::sync::Notify::new());
    let d = done.clone();
    let task_ctx = ctx.clone();
    ctx.run(async move {
        // first-poll re-entry: polling must not hold the lock either
        let _ = task_ctx
            .effect_sync(|| {})
            .expect("polled outside the lock");
        DropRegisters {
            ctx: task_ctx,
            done: d,
        }
    })
    .unwrap();
    bounded(2000, done.notified())
        .await
        .expect("polling and output destruction tripped no framework lock");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drain_joining_a_task_whose_output_drop_reenters_completes() {
    /// Dropped at the task's final await — inside the drain's join window
    /// when the ordering lands as intended — the Drop re-enters
    /// registration on its owning fiber. The generation is already closed
    /// there, so the attempt refuses pre-lock (the EF-09 discrimination
    /// note): this arm pins the no-hang behavior of the drain-join, while
    /// the live-fiber lock-position law is carried by the sibling probe
    /// above and the EF-09 disarm/conversion twins.
    struct DropRegistersOnDrain {
        ctx: Context,
        dropped: Arc<tokio::sync::Notify>,
    }
    impl Drop for DropRegistersOnDrain {
        fn drop(&mut self) {
            let _ = self.ctx.effect_sync(|| {});
            self.dropped.notify_one();
        }
    }

    let parked = Arc::new(tokio::sync::Notify::new());
    let dropped = Arc::new(tokio::sync::Notify::new());
    let release_rx: BarrierSlot<tokio::sync::oneshot::Receiver<()>> = Arc::new(Mutex::new(None));
    let (release_tx, rx) = tokio::sync::oneshot::channel();
    *release_rx.lock() = Some(rx);

    let fiber_handle = spawn_settled(
        &Context::new(),
        Effectful({
            let parked = parked.clone();
            let dropped = dropped.clone();
            let release_rx = release_rx.clone();
            move |ctx: Context| {
                let task_ctx = ctx.clone();
                let parked = parked.clone();
                let dropped = dropped.clone();
                let release = release_rx.lock().take().expect("applied once");
                ctx.run(async move {
                    parked.notify_one();
                    let _ = release.await;
                    DropRegistersOnDrain {
                        ctx: task_ctx,
                        dropped,
                    }
                })?;
                Ok(())
            }
        }),
    )
    .await;
    parked.notified().await;

    // start the drain, let it reach the join (the task is still parked),
    // then release the task so its output drops inside the join window
    let drain = tokio::spawn({
        let fiber_handle = fiber_handle.clone();
        async move { fiber_handle.dispose().await }
    });
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    let _ = release_tx.send(());

    bounded(2000, drain)
        .await
        .expect("the drain-join was not wedged by the output's Drop")
        .unwrap()
        .unwrap();
    bounded(2000, dropped.notified())
        .await
        .expect("the output's Drop ran");
}
