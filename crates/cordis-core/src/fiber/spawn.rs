//! The spawn transaction: admit one typed, sealed [`PreparedPlugin`] and
//! drive a fresh Fiber to live quiescent [`FiberState::Active`](crate::FiberState::Active) or stable
//! [`FiberState::Pending`](crate::FiberState::Pending) before its [`FiberHandle`] is delivered.
//!
//! Creation is one transaction with one commit point and one of three
//! endings:
//!
//! - **handoff** — the fiber settled Active, or is stably Pending on
//!   missing requirements; the caller's FiberHandle is delivered only after the
//!   live quiescent handoff condition is satisfied;
//! - **`InitialApply`** — the first apply returned an error or panicked:
//!   the failed generation's complete LIFO rollback already ran inside
//!   the settle, and the transaction then disposes and unlinks the
//!   attempted Fiber before the error returns, so no resident attempted
//!   Fiber survives a failed creation;
//! - **`Interrupted`** — framework invalidation racing the creation disposed
//!   the attempted Fiber before handoff.
//!   Caller cancellation is never reported as `Interrupted`: dropping
//!   the spawn future after the commit leaves the disposal and unlink to
//!   the [`CreationGuard`], which completes them under framework
//!   ownership, independently of caller polling.
//!
//! The commit point is the residency attach (and the dependency-edge
//! publication beside it). Everything before it — preparation, sealing,
//! the admission gate, the Fiber allocation itself — is synchronous and
//! publishes nothing observable, so a spawn future dropped before the
//! attach just drops the unattached allocation with it: cancellation
//! there has no lifecycle effect by construction. Everything after the
//! commit is driven to one of the endings above.
//!
//! Apply failures are normalized exactly once, at the last boundary that
//! still knows the error's type: the sealed adapter converts
//! `Plugin::ApplyError` into the opaque [`PluginFailure`] (returned-error
//! kind, owned diagnostic text), and the settle boundary converts a
//! contained panic into the same shape (panic kind). The original error
//! object, `Any` access, and downcasts never escape.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::context::Context;
use crate::plugin::PreparedPlugin;
use crate::registry::PluginKey;

use super::{Fiber, FiberHandle, inertia::InitialOutcome};

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum HandoffPhase {
    BeforeRecheck,
    AfterRecheck,
}

#[cfg(test)]
struct HandoffProbe {
    phase: HandoffPhase,
    contract: std::any::TypeId,
    reached: Arc<tokio::sync::Notify>,
    resume: Arc<tokio::sync::Notify>,
}

#[cfg(test)]
static HANDOFF_PROBE: parking_lot::Mutex<Option<Arc<HandoffProbe>>> = parking_lot::Mutex::new(None);

#[cfg(test)]
async fn probe_handoff(phase: HandoffPhase, contract: std::any::TypeId) {
    let probe = HANDOFF_PROBE.lock().clone();
    if let Some(probe) = probe.filter(|probe| probe.phase == phase && probe.contract == contract) {
        probe.reached.notify_one();
        probe.resume.notified().await;
    }
}

/// The semantic kind of a [`PluginFailure`]: the apply returned an error,
/// or it panicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PluginFailureKind {
    /// The apply returned `Err(_)` from its typed `Result`.
    ReturnedError,
    /// The apply panicked; the panic was contained at the settle
    /// boundary.
    Panic,
}

/// One apply attempt's normalized failure: its semantic
/// [`PluginFailureKind`] plus owned diagnostic text.
///
/// Opaque on purpose — no public constructor, no original error object,
/// no downcast: the failure is matched by kind and reported by text,
/// never re-thrown or inspected by type. Normalization happens exactly
/// once, at the boundary that still knows the error's type (see the
/// module docs).
#[derive(Debug)]
pub struct PluginFailure {
    kind: PluginFailureKind,
    diagnostic: String,
}

impl PluginFailure {
    /// The normalized shape of an apply that returned `Err(_)`: the
    /// error's `Display` is the whole diagnostic.
    pub(crate) fn returned(diagnostic: String) -> Self {
        Self {
            kind: PluginFailureKind::ReturnedError,
            diagnostic,
        }
    }

    /// The normalized shape of a contained apply panic: the rendered
    /// payload is the whole diagnostic.
    pub(crate) fn panicked(payload: String) -> Self {
        Self {
            kind: PluginFailureKind::Panic,
            diagnostic: payload,
        }
    }

    /// Whether the apply returned an error or panicked.
    pub fn kind(&self) -> PluginFailureKind {
        self.kind
    }

    /// The owned diagnostic text: the returned error's `Display`, or the
    /// rendered panic payload.
    pub fn diagnostic(&self) -> &str {
        &self.diagnostic
    }
}

impl std::fmt::Display for PluginFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            PluginFailureKind::ReturnedError => {
                write!(f, "returned an error: {}", self.diagnostic)
            }
            PluginFailureKind::Panic => write!(f, "panicked: {}", self.diagnostic),
        }
    }
}

impl std::error::Error for PluginFailure {}

/// Move a parked failure out of its shared cell. The failed fiber's own
/// copy dies with the teardown, so the `Arc` is almost always unique;
/// the clone fallback covers a concurrent `ready()` that borrowed it
/// mid-flight (kind plus owned text — still the one normalization).
pub(crate) fn into_owned(failure: Arc<PluginFailure>) -> PluginFailure {
    match Arc::try_unwrap(failure) {
        Ok(failure) => failure,
        Err(shared) => clone_owned(&shared),
    }
}

/// Copy one already-normalized parked failure without exposing `Clone` as a
/// public capability. Lifecycle operations return owned failure values while
/// the Fiber keeps its authoritative parked copy.
pub(crate) fn clone_owned(failure: &PluginFailure) -> PluginFailure {
    PluginFailure {
        kind: failure.kind,
        diagnostic: failure.diagnostic.clone(),
    }
}

/// Why a [`Context::spawn`] failed. Every variant is a complete answer
/// about the attempted Fiber: on `Err` no FiberHandle is delivered and no
/// resident attempted Fiber remains.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SpawnError {
    /// The spawning Context's fiber generation is closed to new
    /// registrations (stably Pending or Failed, draining, or disposed).
    /// Pre-commit: nothing was allocated.
    #[error("the spawning context's fiber generation is closed to new fibers")]
    InactiveContext,
    /// The first apply returned an error or panicked. The failed
    /// generation's complete LIFO rollback ran, and the attempted Fiber
    /// was disposed and unlinked before this error returned.
    #[error("the initial apply {0}")]
    InitialApply(PluginFailure),
    /// Framework invalidation (a typed group removal racing the creation)
    /// disposed the attempted Fiber after its creation work began but
    /// before FiberHandle handoff; the Fiber is fully disposed and unlinked
    /// before this error returns. Caller cancellation is never reported
    /// as `Interrupted` — it is governed by commit-or-no-effect handoff
    /// ownership (see [`Context::spawn`]).
    #[error("the creation was invalidated by the framework before its FiberHandle was delivered")]
    Interrupted,
}

/// Completion of the creation transaction while the spawn future may be
/// dropped at any await.
///
/// Armed at the residency commit, disarmed at FiberHandle handoff or terminal
/// error delivery — both endings finish the fiber's lifecycle inline, so
/// a disarmed guard is proof the attempted Fiber needs nothing more. An
/// armed guard firing means the caller abandoned the creation mid-flight:
/// the fiber must not be stranded (a resident allocation whose settle
/// pass will never resume), so the teardown is detached through
/// [`crate::effect::detach`]. It stays on the current executor during normal
/// progress and transfers to Cordis's shared completion runtime only if executor
/// shutdown drops the still-pending framework task.
struct CreationGuard {
    fiber: Option<Arc<Fiber>>,
}

impl CreationGuard {
    fn armed(fiber: Arc<Fiber>) -> Self {
        Self { fiber: Some(fiber) }
    }

    /// The creation reached a complete ending (handoff or terminal
    /// error): nothing is left to clean.
    fn disarm(&mut self) {
        self.fiber = None;
    }
}

impl Drop for CreationGuard {
    fn drop(&mut self) {
        let Some(fiber) = self.fiber.take() else {
            return;
        };
        let work = async move {
            fiber.creation_interrupted_teardown().await;
        };
        crate::effect::detach(work);
    }
}

/// The lifecycle admission path fed only by a typed, sealed input.
pub(crate) async fn spawn_prepared(
    ctx: &Context,
    prepared: PreparedPlugin,
) -> std::result::Result<FiberHandle, SpawnError> {
    spawn_prepared_inner(ctx, prepared, true).await
}

/// Replay a captured era creation recipe. The captured spawn-origin Context
/// supplies the immutable view axes, but its Fiber is provenance rather than
/// lifecycle ownership: the source may legitimately outlive that Fiber, so
/// successor admission must not be refused merely because the origin closed.
pub(super) async fn spawn_prepared_era_successor(
    ctx: &Context,
    prepared: PreparedPlugin,
) -> std::result::Result<FiberHandle, SpawnError> {
    spawn_prepared_inner(ctx, prepared, false).await
}

async fn spawn_prepared_inner(
    ctx: &Context,
    prepared: PreparedPlugin,
    require_origin_open: bool,
) -> std::result::Result<FiberHandle, SpawnError> {
    // Ordinary consumer spawn is gated by the calling Context's current
    // generation. Era replay is different: the captured origin is provenance
    // plus view axes, never lifecycle ownership of the replacement.
    if require_origin_open {
        ctx.fiber()
            .assert_can_register()
            .map_err(|_| SpawnError::InactiveContext)?;
    }
    let PreparedPlugin {
        plugin,
        name,
        inject,
        contract,
    } = prepared;
    let plugin_key = PluginKey::Typed(contract);
    let root = &ctx.root;
    let dependency_edges = inject
        .entries()
        .map(|entry| {
            crate::fiber::DependencyEdge::new(entry.name.clone(), ctx.isolate_key(&entry.name))
        })
        .collect::<Vec<_>>();

    let fiber = Fiber::new_with_edges(name.clone(), dependency_edges);
    // the spawn transaction's window: from allocation until handoff (or a
    // teardown), only the initial pass may run the first apply
    fiber.creation_pending.store(true, Ordering::SeqCst);
    // attach under one registry critical section: linearizes against
    // typed group detach (see attach_fiber) — removal racing the spawn
    // either disposes this fiber or leaves it visible, never detached.
    // This is the commit point: from here the creation guard owns the
    // fiber's cleanup if the caller abandons the spawn.
    root.registry.attach_fiber(plugin_key, fiber.clone());
    let mut guard = CreationGuard::armed(fiber.clone());

    // resolve declared injects against this scope now — later audits and
    // SemanticTarget reconstruction must not depend on the spawning context
    // (spawn-time snapshot principle). The stored scope derives the
    // fiber's own scope node (ADR 0003 overturn row 5): every apply —
    // first, restart, dependency reload — derives its context from it, so a
    // fiber's registrations (including typed update-control registrations) stay on
    // its own subtree instead of sharing the spawning scope's pipeline
    // with sibling fibers.
    // Each configured inject row becomes one prepared Service layer on the
    // fiber Context. `resolve_config` composes it after inherited outer layers;
    // restarts retain the same immutable Context view.
    let mut scope = ctx.with_child_scope();
    for entry in inject.entries() {
        if let Some(configured) = &entry.configured {
            scope = scope.with_configured_intercept(entry.name.clone(), configured.clone());
        }
    }
    // capture spawn state for restart and dependency reloads; the era swap's
    // successor re-enters this path with `spawning_ctx` and `plugin_key`,
    // so every era derives its scope from the same captured spawn-origin Scope.
    // Installation is one-shot and nothing installed this fresh fiber
    // before, so a refusal is a framework bug, not a spawn outcome
    // (fail-fast, ADR 0003 §2.4).
    fiber
        .spawn_state
        .install(plugin, name, inject, scope, plugin_key, ctx.clone())
        .expect("a fresh fiber has no spawn state installed");
    // publish the declared edges to the dependency index — both
    // directions, atomically, at the single site where the resolved
    // keys exist (ADR 0016). This line IS the "edges installed =
    // visible to fan-out acceleration. Correctness does not depend on this
    // projection: an unavailable projection falls back to resident Fiber edges.
    root.deps.register(&fiber);
    root.observations
        .publish(crate::observation::RuntimeObservation::FiberResidency {
            change: crate::observation::ResidencyChange::Admitted,
            fiber: crate::observation::fiber_snapshot(root, &fiber),
        });

    let fiber_handle = FiberHandle::new(fiber.clone());
    // claim + initial settle + release live with the rest of the inertia
    // protocol (see InertiaSlot::initial_spawn_pass for why the claim is
    // a CAS and what a lost claim waits out); the pass returns only once
    // the fiber is live quiescent, cleaned, or invalidated.
    match fiber.initial_settle(root).await {
        InitialOutcome::Active | InitialOutcome::Pending => {
            // handoff recheck: the fiber was live quiescent when the pass
            // released the slot, but a framework invalidation's dispose
            // may have been waiting it out. Wait the slot back — the
            // remover's teardown completes under its claim — then judge:
            // alive hands the FiberHandle off; dead reports Interrupted with the
            // teardown already complete (the variant's contract), and the
            // winner's lagging unlink is forced. An invalidation landing
            // after this recheck is indistinguishable from a post-handoff
            // dispose — the commit boundary is here.
            #[cfg(test)]
            probe_handoff(HandoffPhase::BeforeRecheck, contract).await;
            fiber.slot.claim().await;
            let alive = fiber.is_alive();
            fiber.slot.abandon();
            if alive {
                fiber.creation_pending.store(false, Ordering::SeqCst);
                #[cfg(test)]
                probe_handoff(HandoffPhase::AfterRecheck, contract).await;
                guard.disarm();
                Ok(fiber_handle)
            } else {
                fiber.force_unlink();
                guard.disarm();
                Err(SpawnError::Interrupted)
            }
        }
        InitialOutcome::Failed(failure) => {
            // the pass already ran the complete teardown — drain, death
            // mark, edge unregister, unlink — so nothing rides the guard
            guard.disarm();
            Err(SpawnError::InitialApply(failure))
        }
        InitialOutcome::Interrupted => {
            // framework invalidation fully disposed the fiber; the pass
            // made the unlink deterministic before reporting
            guard.disarm();
            Err(SpawnError::Interrupted)
        }
    }
}
impl Context {
    /// Admit one typed, prepared, and sealed Plugin to lifecycle creation
    /// and drive its fresh Fiber to live quiescent [`FiberState::Active`](crate::FiberState::Active)
    /// or stable [`FiberState::Pending`](crate::FiberState::Pending) before returning its [`FiberHandle`].
    ///
    /// This is the whole creation transaction: preparation and sealing
    /// have already completed ([`Plugin::prepare`](crate::Plugin::prepare),
    /// [`PreparedPlugin::from_input`] — both synchronous, ordinary
    /// panics included, under no framework lock and before any
    /// allocation); this method is the first operation allowed to
    /// allocate Runtime state. The returned FiberHandle belongs to a Fiber that
    /// has settled for the current service snapshot: a missing
    /// requirement produces a stable Pending without running apply (a
    /// later publication converges it), never a transient or
    /// mid-convergence state.
    ///
    /// # Errors
    ///
    /// - [`SpawnError::InactiveContext`] — pre-commit refusal: the
    ///   spawning Context's generation is closed; nothing was allocated.
    /// - [`SpawnError::InitialApply`] — the first apply returned an
    ///   error or panicked, normalized exactly once into the opaque
    ///   [`PluginFailure`]. The failed generation's complete LIFO
    ///   rollback ran (publications withdrawn), and the attempted Fiber
    ///   was disposed and unlinked before this error returned: no
    ///   resident attempted Fiber survives a failed creation.
    /// - [`SpawnError::Interrupted`] — framework invalidation racing the
    ///   creation disposed the attempted Fiber before handoff.
    ///
    /// # Cancellation
    ///
    /// Before the allocation/publication commit, dropping this future
    /// has no lifecycle effect (the pre-commit section is synchronous).
    /// After it, the framework completes the transaction independently
    /// of caller polling: the FiberHandle handoff, or the full disposal and
    /// unlink of the undelivered Fiber. Caller cancellation is never
    /// reported as [`SpawnError::Interrupted`].
    pub async fn spawn(
        &self,
        prepared: PreparedPlugin,
    ) -> std::result::Result<FiberHandle, SpawnError> {
        spawn_prepared(self, prepared).await
    }
}

#[cfg(test)]
mod tests {
    use super::{FiberHandle, HANDOFF_PROBE, HandoffPhase, HandoffProbe, spawn_prepared};
    use crate::context::Context;
    use crate::plugin::{Plugin, PreparedPlugin};
    use std::convert::Infallible;
    use std::sync::Arc;

    /// A plugin whose apply parks until the test releases it, so the
    /// remove's dispose queues behind the held initial-pass slot.
    struct ParkedApply {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }

    impl Plugin for ParkedApply {
        type Config = ();
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = Infallible;

        fn prepare(&self, _config: ()) -> Result<(), Infallible> {
            Ok(())
        }

        async fn apply(&self, _ctx: Context, _prepared: &()) -> Result<(), Infallible> {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(())
        }
    }

    struct ProbeReset;

    impl Drop for ProbeReset {
        fn drop(&mut self) {
            *HANDOFF_PROBE.lock() = None;
        }
    }

    async fn handoff_race(phase: HandoffPhase) -> Result<FiberHandle, super::SpawnError> {
        let ctx = Context::new();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release_apply = Arc::new(tokio::sync::Notify::new());
        let reached = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        *HANDOFF_PROBE.lock() = Some(Arc::new(HandoffProbe {
            phase,
            contract: std::any::TypeId::of::<ParkedApply>(),
            reached: reached.clone(),
            resume: resume.clone(),
        }));
        let _reset = ProbeReset;

        let prepared = PreparedPlugin::from_input(
            ParkedApply {
                entered: entered.clone(),
                release: release_apply.clone(),
            },
            (),
        );
        let spawn = {
            let ctx = ctx.clone();
            tokio::spawn(async move { spawn_prepared(&ctx, prepared).await })
        };
        entered.notified().await;
        release_apply.notify_one();
        reached.notified().await;

        // The probe pauses with no framework lock held. Complete the exact
        // same framework invalidation on the chosen side of the recheck.
        let remove_ctx = ctx.clone();
        crate::deadline::bounded(2000, async move {
            remove_ctx.remove_plugins::<ParkedApply>().await.unwrap();
        })
        .await
        .expect("framework invalidation completed");
        assert!(ctx.root.registry.snapshot_fibers().is_empty());

        resume.notify_one();
        crate::deadline::bounded(2000, spawn)
            .await
            .expect("spawn resolved")
            .unwrap()
    }

    async fn cancel_at_handoff_barrier() {
        let ctx = Context::new();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release_apply = Arc::new(tokio::sync::Notify::new());
        let reached = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        *HANDOFF_PROBE.lock() = Some(Arc::new(HandoffProbe {
            phase: HandoffPhase::BeforeRecheck,
            contract: std::any::TypeId::of::<ParkedApply>(),
            reached: reached.clone(),
            resume,
        }));
        let _reset = ProbeReset;

        let prepared = PreparedPlugin::from_input(
            ParkedApply {
                entered: entered.clone(),
                release: release_apply.clone(),
            },
            (),
        );
        let spawn = {
            let ctx = ctx.clone();
            tokio::spawn(async move { spawn_prepared(&ctx, prepared).await })
        };
        entered.notified().await;
        release_apply.notify_one();
        reached.notified().await;

        // Same deterministic pre-handoff barrier as the Interrupted case,
        // but this time the caller disappears. There is no SpawnError to
        // report: task cancellation drops the creation future, whose guard
        // takes framework ownership of disposal and exact unlink.
        spawn.abort();
        assert!(spawn.await.unwrap_err().is_cancelled());
        crate::deadline::bounded(2000, async {
            while !ctx.root.registry.snapshot_fibers().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("caller cancellation completed disposal and unlink");
    }

    /// The handoff barrier discriminates framework invalidation from caller
    /// cancellation without scheduler timing: only invalidation can surface
    /// `Interrupted`, while cancellation completes cleanup under framework
    /// ownership and reports nothing through the abandoned spawn future.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handoff_barrier_discriminates_invalidation_from_caller_cancellation() {
        assert!(matches!(
            handoff_race(HandoffPhase::BeforeRecheck).await,
            Err(super::SpawnError::Interrupted)
        ));

        cancel_at_handoff_barrier().await;

        let handed_off = handoff_race(HandoffPhase::AfterRecheck)
            .await
            .expect("invalidation after the commit cannot become Interrupted");
        assert_eq!(handed_off.state(), crate::FiberState::Disposed);

        era_framework_invalidation_is_successor_lost_after_complete_unlink().await;
    }

    struct EraHandoffProbe {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
        cleaned: Arc<std::sync::atomic::AtomicBool>,
    }

    impl Plugin for EraHandoffProbe {
        type Config = u8;
        type Input = u8;
        type PrepareError = Infallible;
        type ApplyError = Infallible;

        fn prepare(&self, config: u8) -> Result<u8, Infallible> {
            Ok(config)
        }

        async fn apply(&self, ctx: Context, input: &u8) -> Result<(), Infallible> {
            if *input == 2 {
                let cleaned = self.cleaned.clone();
                ctx.effect_sync(move || {
                    cleaned.store(true, std::sync::atomic::Ordering::SeqCst);
                })
                .unwrap();
                self.entered.notify_one();
                self.release.notified().await;
            }
            Ok(())
        }
    }

    async fn era_framework_invalidation_is_successor_lost_after_complete_unlink() {
        let ctx = Context::new();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release_apply = Arc::new(tokio::sync::Notify::new());
        let reached = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let cleaned = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let source = ctx
            .spawn(PreparedPlugin::from_input(
                EraHandoffProbe {
                    entered: entered.clone(),
                    release: release_apply.clone(),
                    cleaned: cleaned.clone(),
                },
                1,
            ))
            .await
            .unwrap();
        *HANDOFF_PROBE.lock() = Some(Arc::new(HandoffProbe {
            phase: HandoffPhase::BeforeRecheck,
            contract: std::any::TypeId::of::<EraHandoffProbe>(),
            reached: reached.clone(),
            resume: resume.clone(),
        }));
        let _reset = ProbeReset;

        let swap = tokio::spawn(async move {
            source
                .era_swap(crate::PreparedChange::from_input::<EraHandoffProbe>(2))
                .await
        });
        entered.notified().await;
        release_apply.notify_one();
        reached.notified().await;

        crate::deadline::bounded(2000, ctx.remove_plugins::<EraHandoffProbe>())
            .await
            .expect("framework invalidation completed")
            .unwrap();
        assert!(cleaned.load(std::sync::atomic::Ordering::SeqCst));
        assert!(ctx.root.registry.snapshot_fibers().is_empty());

        resume.notify_one();
        let error = crate::deadline::bounded(2000, swap)
            .await
            .expect("era replacement resolved")
            .unwrap()
            .unwrap_err();
        assert!(matches!(
            error,
            crate::fiber::EraSwapError::Incomplete(crate::fiber::EraSwapFailure::SuccessorLost)
        ));
        assert!(ctx.root.registry.snapshot_fibers().is_empty());
    }
}
