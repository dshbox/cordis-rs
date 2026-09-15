//! Fiber — the lifecycle carrier of a plugin instance (mirrors `fiber.ts`).
//!
//! - A fiber owns a set of disposables (effects) registered by its plugin.
//! - The **SemanticTarget** combines committed apply input with the exact
//!   publication assignment of every fixed dependency slot. Target drift
//!   drives `unload → reload`; the protocol lives in the private `InertiaSlot`.
//! - [`FiberHandle`] is the public handle returned by
//!   [`Context::spawn`](crate::Context::spawn); the `Fiber` body is
//!   `pub(crate)` so the public-handle/private-Fiber split is enforced by
//!   visibility, not convention (ADR 0004).
//!
//! ## File map
//!
//! | file | contents |
//! |---|---|
//! | `mod` | `Fiber` (private), [`FiberState`], [`FiberHandle`], [`Context::run`] |
//! | `spawn` | the spawn transaction — [`Context::spawn`], [`SpawnError`], [`PluginFailure`], and the creation-cancellation guard |
//! | `spawn_state` | `SpawnState` — the installed spawn record, lock discipline, and purpose-specific snapshots |
//! | `inertia` | `InertiaSlot` — the settle protocol's single home: the ops, the `SemanticTarget` cell, and the three lifecycle passes (initial spawn, convergence, restart) |
//! | `settle_ctx` | the settle context — attribution and the lifecycle-entry refusal (ADR 0019) |
//! | `era` | [`FiberHandle::era_swap`] — one live-source claim, full old terminal barrier, fresh sibling-era successor, and final convergence (ADR 0030) |

mod era;
mod inertia;
mod settle_ctx;
mod spawn;
mod spawn_state;

use crate::context::{Context, Root};
use crate::effect::{Cleanup, DisposableList, TaskRegistrationError};
use crate::registry::ResidencyClaim;
use parking_lot::Mutex;
use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Opaque, Runtime-local correlation identity for one Fiber.
///
/// An ID carries no lookup or lifecycle authority. Its representation is
/// deliberately private; only equality, hashing, cloning, and debugging are
/// semantic operations.
#[derive(Clone)]
pub struct FiberId(Arc<u8>);

impl PartialEq for FiberId {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for FiberId {}
impl std::hash::Hash for FiberId {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self.0), state);
    }
}

impl fmt::Debug for FiberId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FiberId(..)")
    }
}
use std::time::Duration;

pub(crate) use inertia::InertiaSlot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GenerationClosed;
use inertia::SemanticTarget;
pub use spawn::{PluginFailure, PluginFailureKind, SpawnError};
use spawn_state::SpawnState;

/// Semantic role of a Fiber in a Runtime snapshot.
///
/// This classification exposes no Registry grouping or lifecycle authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FiberRole {
    /// The Runtime's one permanent root Fiber.
    Root,
    /// An ordinary Registry-resident Fiber.
    Ordinary,
}

/// Published lifecycle state of a Fiber.
///
/// The state is semantic rather than representational: it has no default,
/// numeric representation, ordering, or serialization contract. A transition
/// becomes observable only at the lifecycle barrier documented by the variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FiberState {
    /// The current generation is open and its Plugin apply may register effects.
    Loading,
    /// The current generation applied successfully and its effects are live.
    Active,
    /// The current SemanticTarget has a Missing assignment; no apply is live.
    Pending,
    /// The generation gate is closed and owned cleanup is in progress.
    Unloading,
    /// Apply failed and the failed generation's rollback has completed.
    Failed,
    /// Terminal generation cleanup has completed; the Fiber cannot activate again.
    Disposed,
}

impl FiberState {
    /// Private index used only by the reliable-publication bookkeeping. This is
    /// deliberately a match, not a public discriminant or representation.
    fn publication_index(self) -> usize {
        match self {
            Self::Loading => 0,
            Self::Active => 1,
            Self::Pending => 2,
            Self::Unloading => 3,
            Self::Failed => 4,
            Self::Disposed => 5,
        }
    }
}

struct StatePublicationInner {
    current: FiberState,
    revisions: [u64; 6],
}

/// Reliable state-publication cell. `Notify` is only a wake hint; per-state
/// revisions are the durable observation truth, so a waiter cannot miss a
/// requested transient publication even when another state is published before
/// it is polled again.
struct StatePublication {
    inner: Mutex<StatePublicationInner>,
    changed: tokio::sync::Notify,
}

impl StatePublication {
    fn new(initial: FiberState) -> Self {
        let mut revisions = [0; 6];
        revisions[initial.publication_index()] = 1;
        Self {
            inner: Mutex::new(StatePublicationInner {
                current: initial,
                revisions,
            }),
            changed: tokio::sync::Notify::new(),
        }
    }

    fn current(&self) -> FiberState {
        self.inner.lock().current
    }

    fn publish(&self, next: FiberState) -> Option<FiberState> {
        let old = {
            let mut inner = self.inner.lock();
            let old = inner.current;
            if old == next {
                return None;
            }
            inner.current = next;
            let revision = &mut inner.revisions[next.publication_index()];
            *revision = revision
                .checked_add(1)
                .expect("state publication revision overflow");
            old
        };
        Some(old)
    }

    fn notify(&self) {
        self.changed.notify_waiters();
    }

    fn snapshot(&self, target: FiberState) -> (FiberState, u64) {
        let inner = self.inner.lock();
        (inner.current, inner.revisions[target.publication_index()])
    }

    async fn wait_for(
        &self,
        target: FiberState,
        timeout: Duration,
    ) -> std::result::Result<(), WaitStateError> {
        let (_, baseline_revision) = self.snapshot(target);
        self.wait_for_after(target, baseline_revision, timeout)
            .await
    }

    async fn wait_for_after(
        &self,
        target: FiberState,
        baseline_revision: u64,
        timeout: Duration,
    ) -> std::result::Result<(), WaitStateError> {
        let (current, revision) = self.snapshot(target);
        if current == target || revision != baseline_revision {
            return Ok(());
        }

        let deadline = crate::deadline::watchdog(timeout);
        tokio::pin!(deadline);
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            let (current, revision) = self.snapshot(target);
            if current == target || revision != baseline_revision {
                return Ok(());
            }

            tokio::select! {
                _ = &mut deadline => return Err(WaitStateError::Elapsed),
                _ = &mut notified => {}
            }
        }
    }
}

#[cfg(test)]
mod state_publication_tests {
    use super::{FiberState, StatePublication};
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn requested_publication_survives_state_notification_race() {
        let state = StatePublication::new(FiberState::Pending);
        let (_, before_loading) = state.snapshot(FiberState::Loading);

        state.publish(FiberState::Loading);
        state.notify();
        state.publish(FiberState::Active);
        state.notify();

        state
            .wait_for_after(
                FiberState::Loading,
                before_loading,
                Duration::from_millis(20),
            )
            .await
            .expect("the durable Loading revision survives a superseding Active publication");
        assert_eq!(state.current(), FiberState::Active);
    }

    #[tokio::test]
    async fn completed_waits_do_not_hold_watchdogs_until_timeout() {
        let timeout = crate::deadline::tracked_watchdog_timeout();
        let before = crate::deadline::live_watchdog_threads();
        let immediate = StatePublication::new(FiberState::Active);

        for _ in 0..20 {
            immediate
                .wait_for(FiberState::Active, timeout)
                .await
                .expect("the already-published target resolves immediately");
        }
        assert_eq!(
            crate::deadline::live_watchdog_threads(),
            before,
            "immediately-satisfied waits must not leave watchdog threads",
        );

        let state = Arc::new(StatePublication::new(FiberState::Pending));
        let waiter = {
            let state = state.clone();
            tokio::spawn(async move { state.wait_for(FiberState::Active, timeout).await })
        };

        for _ in 0..100 {
            if crate::deadline::live_watchdog_threads() == before + 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(crate::deadline::live_watchdog_threads(), before + 1);

        state.publish(FiberState::Active);
        state.notify();
        waiter
            .await
            .expect("waiter task stays alive")
            .expect("the publication satisfies the wait");

        for _ in 0..25 {
            if crate::deadline::live_watchdog_threads() == before {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            crate::deadline::live_watchdog_threads(),
            before,
            "an armed watchdog must exit when the wait resolves early",
        );
    }
}

/// Successor-specific cause of a committed era replacement that could not hand off a fresh Fiber.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EraSwapFailure {
    /// A successor dependency contract could not be admitted.
    #[error("successor dependency `{service}` has a contract mismatch")]
    SuccessorDependencyContractMismatch {
        /// The dependency Service name.
        service: String,
    },
    /// A successor dependency configuration could not be prepared.
    #[error("successor dependency `{service}` configuration is invalid: {diagnostic}")]
    SuccessorDependencyConfiguration {
        /// The dependency Service name.
        service: String,
        /// The normalized preparation diagnostic.
        diagnostic: String,
    },
    /// The fresh successor's initial apply failed after allocation.
    #[error("successor apply failed: {0}")]
    SuccessorApply(PluginFailure),
    /// Framework invalidation prevented delivery of the fresh successor.
    #[error("the fresh successor was lost before handoff")]
    SuccessorLost,
}

/// Failure of one identity-breaking fresh-era replacement attempt.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EraSwapError {
    /// Candidate belongs to another Plugin contract.
    #[error("prepared change targets a different plugin contract")]
    PluginContractMismatch,
    /// The source Fiber was already closing or terminal before the source claim.
    #[error("fiber is closed")]
    Closed,
    /// Lifecycle recursion was refused before the source claim.
    #[error("{0}")]
    Recursion(LifecycleRecursion),
    /// The source claim committed, but no successor can be handed off.
    #[error("era replacement incomplete: {0}")]
    Incomplete(EraSwapFailure),
}

/// Successful outcome of one typed same-Fiber update attempt.
///
/// Precommit control may veto without reaching lifecycle commit. Once commit is
/// admitted, the final transformed candidate is authoritative and completion
/// reports the resulting quiescent Active or stable Pending state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateOutcome {
    /// The final accepted candidate committed on this same Fiber.
    Committed(FiberState),
    /// Precommit control returned successfully without reaching its private tail;
    /// the old authoritative generation remains unchanged.
    Vetoed,
}

/// Failure of one typed same-Fiber update attempt.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum UpdateError {
    /// Candidate belongs to another Plugin contract.
    #[error("prepared change targets a different plugin contract")]
    PluginContractMismatch,
    /// The target was already terminal at update entry.
    #[error("fiber is closed")]
    Closed,
    /// Same-Fiber lifecycle recursion was refused.
    #[error("{0}")]
    Recursion(LifecycleRecursion),
    /// A precommit Mapper/Around callback failed.
    #[error("update control failed: {0}")]
    Control(crate::events::InvocationFailure),
    /// Awaited control completed after lifecycle admission became terminal.
    #[error("fiber lost update admission before commit")]
    AdmissionLost,
    /// The candidate committed, but applying its generation failed.
    #[error("plugin apply failed: {0}")]
    Apply(PluginFailure),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DependencyEdge {
    pub(crate) service: String,
    pub(crate) realm: crate::context::RealmKey,
}

impl DependencyEdge {
    pub(crate) fn new(service: String, realm: crate::context::RealmKey) -> Self {
        Self { service, realm }
    }
}

/// Internal lifecycle state shared by the FiberHandle clones for one Fiber.
///
/// `pub(crate)` on purpose (ADR 0004): callers receive the opaque [`FiberHandle`]
/// control handle and [`FiberId`] correlation value, never the Fiber body.
pub(crate) struct Fiber {
    id: FiberId,
    /// Lifecycle liveness is separate from opaque correlation identity.
    alive: AtomicBool,
    /// Terminal Open-to-Closing claim. Besides arbitration, this is a
    /// synchronous generation-gate close: gated publication rechecks it so
    /// nothing can commit after terminal disposal has linearized.
    disposing: AtomicBool,
    /// A same-Fiber generation replacement has committed but its detached
    /// owner may not have published `Unloading` yet. This closes the same tiny
    /// admission window for restart/update replacement without conflating it
    /// with terminal Fiber liveness.
    generation_replacing: AtomicBool,
    /// Full terminal barrier publication: set only after cleanup, Disposed,
    /// exact residency release, and conditional Registry prune have finished.
    dispose_complete: AtomicBool,
    dispose_done: tokio::sync::Notify,
    /// Display name resolved from the plugin name.
    pub(crate) name: String,
    /// Immutable exact dependency slots resolved once for this Fiber era.
    dependency_edges: Box<[DependencyEdge]>,
    /// Current lifecycle state plus reliable publication history.
    state: StatePublication,
    /// Effects registered during `apply`; drained on every unload.
    pub(crate) disposables: Mutex<DisposableList>,
    /// The settle protocol slot. Everything about when this fiber may
    /// settle, what target it settles toward, and who owes a wakeup lives
    /// in [`InertiaSlot`] — never touch its fields outside that impl.
    pub(crate) slot: InertiaSlot,
    /// The normalized failure of the last failed apply, if any (set iff
    /// the last settle ended Failed). Normalization happens once, at the
    /// boundary that still knew the error's type (see `spawn`).
    pub(crate) error: Mutex<Option<Arc<PluginFailure>>>,
    /// Deep module owning the optional spawn-time record, its lock, and all
    /// purpose-specific snapshots (ADR 0023).
    pub(crate) spawn_state: SpawnState,
    /// The exact private Registry residency capability for this Fiber.
    pub(crate) residency: Mutex<Option<ResidencyClaim>>,
    /// Drain-in-flight mark: set across [`Fiber::drain_disposables`],
    /// where the fiber's `Context::run` tasks are being joined. Read by
    /// the settle-context refusal (ADR 0019) — an observer entry called
    /// from one of those tasks is waiting out a drain that awaits it.
    draining: AtomicBool,
    /// The first apply has run (any outcome). With `creation_pending`,
    /// gates first-apply ownership: only the creation's initial pass may
    /// run it, so a convergence that wins the slot in the spawn window
    /// can neither double-apply nor swallow a first-apply failure.
    pub(crate) applied: AtomicBool,
    /// Set from allocation until the spawn transaction's handoff (or a
    /// teardown). See `applied`.
    pub(crate) creation_pending: AtomicBool,
}

impl std::fmt::Debug for Fiber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fiber")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("state", &self.state())
            .finish()
    }
}

impl Fiber {
    /// Allocate one fresh Fiber with an unforgeable correlation identity.
    pub(crate) fn new(name: impl Into<String>) -> Arc<Self> {
        Self::new_with_edges(name, Vec::new())
    }

    pub(crate) fn new_with_edges(
        name: impl Into<String>,
        dependency_edges: Vec<DependencyEdge>,
    ) -> Arc<Self> {
        Arc::new(Self {
            id: FiberId(Arc::new(0)),
            alive: AtomicBool::new(true),
            disposing: AtomicBool::new(false),
            generation_replacing: AtomicBool::new(false),
            dispose_complete: AtomicBool::new(false),
            dispose_done: tokio::sync::Notify::new(),
            name: name.into(),
            dependency_edges: dependency_edges.into_boxed_slice(),
            state: StatePublication::new(FiberState::Pending),
            disposables: Mutex::new(DisposableList::new()),
            slot: InertiaSlot::new(),
            error: Mutex::new(None),
            spawn_state: SpawnState::new(),
            residency: Mutex::new(None),
            draining: AtomicBool::new(false),
            applied: AtomicBool::new(false),
            // the spawn transaction marks its own window; fibers built
            // any other way (the root, test fixtures) have none
            creation_pending: AtomicBool::new(false),
        })
    }

    /// The root fiber: permanently ACTIVE, no spawn state, and no dependency edges.
    pub(crate) fn root() -> Arc<Self> {
        let fiber = Self::new("root");
        fiber.set_state(FiberState::Active);
        fiber
    }

    pub(crate) fn id(&self) -> &FiberId {
        &self.id
    }

    pub(crate) fn dependency_edges(&self) -> &[DependencyEdge] {
        &self.dependency_edges
    }

    /// Current published lifecycle state.
    pub(crate) fn state(&self) -> FiberState {
        self.state.current()
    }

    /// Construction-only state publication. Runtime transitions go through
    /// [`Self::transition`] so visibility drift and postcommit observation keep
    /// their ordering.
    pub(crate) fn set_state(&self, state: FiberState) {
        self.state.publish(state);
    }

    /// Publish one lifecycle transition, commit any Service-visibility drift,
    /// wake reliable state waiters, then publish immutable Runtime observation
    /// records for the committed transition. No-op transitions emit nothing.
    /// Construction-time transitions (`Fiber::new`/`Fiber::root`) keep
    /// using `set_state` directly — no listener can exist that early.
    pub(crate) async fn transition(self: &Arc<Self>, next: FiberState) {
        let old = self.state();
        if old == next {
            return;
        }
        // Service visibility is a lifecycle fact, not declared Plugin metadata.
        // Active-boundary transitions publish the provider state and the complete
        // dependent recheck obligation in one ServiceStore semantic commit
        // (ADR 0031). Non-visibility transitions keep the direct state write.
        let root = self.spawn_state.root();
        let (visibility, drift) = if (old == FiberState::Active) != (next == FiberState::Active) {
            match root.as_ref() {
                Some(root) => root.services.commit_fiber_transition(root, self, old, next),
                None => {
                    self.set_state(next);
                    (Vec::new(), None)
                }
            }
        } else {
            self.set_state(next);
            (Vec::new(), None)
        };
        if let (Some(root), Some(drift)) = (root.as_ref(), drift) {
            // Kicks are acceleration only and deliberately run after releasing
            // ServiceStore synchronization.
            drift.kick(root);
        }

        // Reliable state wakeups are protocol signals, separate from optional
        // Runtime observation. Revisions were committed with the state write;
        // the wake comes only after the state transition's durable follow-up.
        self.state.notify();

        if let Some(root) = root {
            for slot in visibility {
                let id = crate::observation::ServicePublicationId(slot.occurrence);
                root.observations.publish(
                    crate::observation::RuntimeObservation::ServiceVisibility {
                        service: slot.service,
                        realm: crate::ServiceRealm::new(root.realm_membership.clone(), slot.realm),
                        previous: (old == FiberState::Active).then(|| id.clone()),
                        current: (next == FiberState::Active).then_some(id),
                    },
                );
            }
            root.observations
                .publish(crate::observation::RuntimeObservation::FiberState {
                    fiber: self.id().clone(),
                    previous: old,
                    current: next,
                });
        }
    }

    /// This fiber's own context (spawn scope + fiber handle), while the
    /// spawn state is known. The root fiber has none.
    pub(crate) fn fiber_ctx(self: &Arc<Self>) -> Option<Context> {
        self.spawn_state.context_for(self)
    }

    /// Whether this fiber can still create effects, i.e. it has not been
    /// disposed. Deliberately *not* named `is_active`: that name collides
    /// with [`FiberState::Active`], while this predicate is true for
    /// Pending/Loading/Failed fibers too.
    pub(crate) fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// Error out when creating effects on a disposed fiber (mirrors TS
    /// `assertActive`, fiber.ts:224-227).
    pub(crate) fn assert_alive(&self) -> std::result::Result<(), GenerationClosed> {
        if self.is_alive() {
            Ok(())
        } else {
            Err(GenerationClosed)
        }
    }

    /// Fail fast on *new registrations* against a fiber whose generation
    /// is closed: stable Pending (never applied or converged inactive),
    /// stable Failed (parked after rollback), Unloading (mid-reload or
    /// mid-dispose), or already gone. Only Loading and Active admit —
    /// registration anywhere else would either strand its cleanup behind
    /// an already-taken drain snapshot or leak the obligation into the
    /// next generation's journal. Lifecycle ops (`FiberHandle::restart`) keep
    /// plain `assert_alive`: they serialize through the inertia slot
    /// instead of refusing. The root fiber's generation stays open for
    /// the runtime's duration (it is permanently Active).
    pub(crate) fn assert_can_register(&self) -> std::result::Result<(), GenerationClosed> {
        self.assert_alive()?;
        if self.disposing.load(Ordering::SeqCst) || self.generation_replacing.load(Ordering::SeqCst)
        {
            return Err(GenerationClosed);
        }
        match self.state() {
            FiberState::Loading | FiberState::Active => Ok(()),
            FiberState::Pending
            | FiberState::Unloading
            | FiberState::Failed
            | FiberState::Disposed => Err(GenerationClosed),
        }
    }

    /// Store (or clear) the parked-failure cell. The cell holds only
    /// normalized framework-owned data (kind plus owned diagnostic), so
    /// no user destruction can run under its lock.
    fn store_error(&self, next: Option<Arc<PluginFailure>>) {
        *self.error.lock() = next;
    }

    /// Take the parked failure out of the cell — the spawn path's move
    /// of the initial apply's failure into [`SpawnError::InitialApply`].
    pub(crate) fn take_error(&self) -> Option<Arc<PluginFailure>> {
        self.error.lock().take()
    }

    /// Names of declared dependencies currently missing from the service
    /// store. Startup diagnostics report these for PENDING fibers (feeds
    /// `pending_missing`). The authoritative exact edges are owned by this Fiber.
    pub(crate) fn missing_from(&self, root: &Root) -> Vec<String> {
        self.dependency_edges
            .iter()
            .filter(|edge| {
                root.services
                    .occurrence_id(&edge.realm, &edge.service)
                    .is_none()
            })
            .map(|edge| edge.service.clone())
            .collect()
    }

    /// Run one claimed cleanup and report its failure, if any. The
    /// execution itself ([`crate::effect::execute_cleanup`]) contains
    /// panics and normalizes returned errors into one
    /// [`EffectFailure`](crate::effect::EffectFailure); here the failure
    /// becomes a diagnostic on the fiber's logger (stderr fallback) and
    /// the drain moves on — a cleanup failure is always per-item and
    /// secondary: it never replaces the lifecycle operation's own error
    /// and never strands the remaining cleanups (service withdrawals,
    /// listener removals, timer cancellation).
    ///
    /// The reporting channel is resolved before the user code runs:
    /// spawn_state is stable across the drain, so this is observably the
    /// same logger an execution-time lookup would find — the root fiber
    /// has no spawn state and keeps the stderr fallback.
    pub(crate) async fn run_cleanup_contained(self: &Arc<Self>, cleanup: Cleanup) {
        let logger = self.fiber_ctx().map(|ctx| ctx.logger());
        // Once claimed, execution is framework-owned: the cleanup runs on
        // a detached task — the current runtime's, or off-runtime one
        // dedicated thread carrying its own Tokio runtime, so
        // Tokio-touching cleanups run as written — and only the outcome
        // channel is awaited. A cancelled drain (a spawn future dropped
        // mid-rollback) abandons the wait, never the half-run cleanup
        // (LF-07's attempt-and-complete law). The detached execution
        // reinstalls the settle bracket, so lifecycle entries from inside
        // the cleanup refuse exactly as inline execution did (ADR 0019).
        let (tx, rx) = tokio::sync::oneshot::channel();
        let fiber = self.clone();
        let report_logger = logger.clone();
        let work = async move {
            let outcome = settle_ctx::bracket(fiber, async move {
                crate::effect::execute_cleanup(cleanup).await
            })
            .await;
            if let Err(outcome) = tx.send(outcome)
                && let Some(failure) = outcome
            {
                // nobody is left to receive it — the exactly-one
                // diagnostic report still lands
                crate::effect::report_cleanup_failure(report_logger.as_ref(), &failure);
            }
        };
        crate::effect::detach(work);
        // `Err` means the send side died unsent: the driving executor
        // itself was torn down mid-cleanup — the claim was still won and
        // owned (same note as `EffectRegistration::dispose`)
        if let Ok(Some(failure)) = rx.await {
            crate::effect::report_cleanup_failure(logger.as_ref(), &failure);
        }
    }

    /// Run the cleanups the given tokens resolve to, **LIFO**, each
    /// exactly once (claimed before it runs) and contained: a failing or
    /// panicking cleanup is normalized, reported, and the sequence
    /// continues, so one broken cleanup never strands the rest. Tokens
    /// that no longer resolve (early manual claims) are skipped.
    pub(crate) async fn run_tokens_contained(
        self: &Arc<Self>,
        tokens: Vec<crate::effect::DisposableToken>,
    ) {
        for token in tokens.into_iter().rev() {
            if let Some(cleanup) = self.remove_disposable(token) {
                self.run_cleanup_contained(cleanup).await;
            }
        }
    }

    /// Remove every obligation (LIFO) and run each exactly once, each
    /// contained. Tokens are claimed *before* their cleanup runs, so an
    /// effect cannot execute twice.
    ///
    /// The drain mark is live across the whole run: `Context::run` tasks
    /// are being joined in here, so their observer entries on this fiber
    /// must refuse instead of waiting out a drain that awaits them
    /// (ADR 0019).
    pub(crate) async fn drain_disposables(self: &Arc<Self>) {
        let tokens: Vec<_> = self.disposables.lock().tokens();
        self.draining.store(true, Ordering::SeqCst);
        self.run_tokens_contained(tokens).await;
        self.draining.store(false, Ordering::SeqCst);
    }

    /// Reconstruct the exact semantic settle target from Fiber-owned edges,
    /// committed effective input, and current visible publication assignments.
    fn compute_target(&self, root: &Root) -> SemanticTarget {
        let input = self
            .spawn_state
            .effective_input()
            .expect("settled plugin fibers have committed effective input");
        let assignments = self
            .dependency_edges
            .iter()
            .map(|edge| {
                let publication = root.services.occurrence_id(&edge.realm, &edge.service);
                (edge.clone(), publication)
            })
            .collect();
        SemanticTarget::new(input, assignments)
    }

    /// Best-effort driver for an already committed Service target recheck.
    /// Durable truth was recorded before this entry; executor availability
    /// and single-driver ownership live in [`InertiaSlot::kick`](inertia::InertiaSlot::kick).
    pub(crate) fn kick_committed_recheck(self: &Arc<Self>, root: &Arc<Root>) {
        self.slot.kick(self, root);
    }

    /// One settle pass: tear down whatever is live, then (re-)apply
    /// toward `target`. Shared by initial spawn, restart, and the
    /// convergence loop.
    ///
    /// `owns_first_apply` is `true` only for the creation's initial pass:
    /// while the spawn transaction is pending, a convergence pass that
    /// won the slot in the spawn window must leave the first apply to
    /// that pass — re-running it would double-apply, and a won-back
    /// success would swallow a first-apply failure that is the creation's
    /// `InitialApply` ending (LF-01/LF-07).
    ///
    /// Refuses to run on a disposed fiber (a dispose must be final even if
    /// a racing notification re-computes a all-present SemanticTarget),
    /// converts an `apply` panic into the Failed state instead of letting
    /// it escape — otherwise the fiber would stay LOADING forever and
    /// [`FiberHandle::ready`](FiberHandle::ready) would hang — and tears down whatever a
    /// failed apply registered before settling on Failed. Upstream's
    /// `_reload` catch reaches the same teardown through its inactive state;
    /// this port drains directly but deliberately keeps the exact failed
    /// SemanticTarget parked (ADR 0031). Same-target notifications are absorbed;
    /// a changed effective input or publication assignment can settle again, and
    /// explicit restart retains its same-target bypass.
    async fn settle_once(self: &Arc<Self>, target: &SemanticTarget, owns_first_apply: bool) {
        // the settle bracket: every user-code phase below (apply bodies and
        // cleanups) runs while this
        // fiber's slot is held, so lifecycle entries on this fiber
        // refuse from inside it instead of waiting on the pass that must
        // first return (ADR 0019)
        settle_ctx::bracket(
            self.clone(),
            self.settle_once_inner(target, owns_first_apply),
        )
        .await
    }

    async fn settle_once_inner(self: &Arc<Self>, target: &SemanticTarget, owns_first_apply: bool) {
        if !self.is_alive() {
            return; // disposed: never re-apply
        }
        match self.state() {
            FiberState::Active | FiberState::Failed | FiberState::Loading => {
                self.transition(FiberState::Unloading).await;
                self.drain_disposables().await;
            }
            // Pending: never applied, no effects to drain. Unloading: the
            // drain is already owned by whoever set that state (reload or
            // dispose) — running a second one here would double-dispose.
            // Disposed: unreachable through the liveness guard above, kept
            // explicit so the match stays exhaustive. Every arm is spelled
            // out so a future FiberState variant breaks this match at
            // compile time instead of silently landing in a no-op
            // (fail-fast, STATE_TABLE posture).
            FiberState::Pending | FiberState::Unloading | FiberState::Disposed => {}
        }
        if target.has_missing() {
            // Pending attests no apply outcome. Clear any stale parked failure
            // before publishing the state so state/error meaning is already
            // coherent when observers see Pending.
            self.store_error(None);
            self.transition(FiberState::Pending).await;
            return;
        }
        // First-apply ownership (LF-01): while the spawn transaction is
        // pending, only its initial pass may run the first apply. A
        // convergence pass that won the slot in that window leaves the
        // fiber Pending — the durable committed recheck keeps drift visible,
        // and the waiting initial pass settles toward the live target.
        if !owns_first_apply
            && self.creation_pending.load(Ordering::SeqCst)
            && !self.applied.load(Ordering::SeqCst)
        {
            return;
        }
        match self.spawn_state.apply_snapshot() {
            Ok(state) => {
                // A committed restart/update closes old-generation admission
                // synchronously before its detached owner starts. Re-open only
                // at the new generation's Loading boundary, before user apply
                // may register anything into its fresh journal.
                self.generation_replacing.store(false, Ordering::SeqCst);
                self.transition(FiberState::Loading).await;
                let ctx = state.scope.with_fiber(self.clone());
                // Containment is the boundary's catch-and-convert policy: a
                // panicking apply becomes the Failed state below instead of
                // unwinding through the settle loop (which would leave the
                // fiber LOADING forever and hang `FiberHandle::ready`).
                // AssertUnwindSafe posture: plugin state may be inconsistent
                // after the panic, which is exactly why the fiber goes to
                // FAILED and its error is recorded rather than retried
                // blindly.
                let outcome = crate::contained::catch_contained(
                    state.plugin.clone().apply_boxed(ctx.clone()),
                )
                .await;
                self.applied.store(true, Ordering::SeqCst);
                match outcome {
                    Ok(Ok(())) => {
                        self.store_error(None);
                        self.transition(FiberState::Active).await;
                    }
                    Ok(Err(failure)) => {
                        // Close the generation gate before rollback begins.
                        // Failed is a post-rollback publication, never the
                        // rollback-in-progress marker.
                        self.transition(FiberState::Unloading).await;
                        self.drain_disposables().await;
                        self.store_error(Some(Arc::new(failure)));
                        self.transition(FiberState::Failed).await;
                    }
                    Err(panic) => {
                        self.transition(FiberState::Unloading).await;
                        self.drain_disposables().await;
                        self.store_error(Some(Arc::new(PluginFailure::panicked(
                            crate::contained::payload_text(&panic),
                        ))));
                        self.transition(FiberState::Failed).await;
                    }
                }
            }
            Err(_) => {
                unreachable!("settling a plugin Fiber requires installed spawn state")
            }
        }
    }

    /// Drive the initial settle pass of a freshly spawned fiber — the
    /// claim choreography and its justifications live in
    /// [`InertiaSlot::initial_spawn_pass`](inertia::InertiaSlot::initial_spawn_pass);
    /// this method keeps the spawn path's entry point on `Fiber`, where
    /// the spawn transaction calls it. The outcome tells the transaction
    /// which ending the creation reached.
    pub(crate) async fn initial_settle(
        self: &Arc<Self>,
        root: &Arc<Root>,
    ) -> inertia::InitialOutcome {
        self.slot.initial_spawn_pass(self, root).await
    }

    /// Complete the terminal disposal transaction. The first caller commits
    /// Open-to-Closing while holding the lifecycle slot, then transfers the
    /// remainder to framework-owned detached work. Every racing/later caller
    /// waits for the same full cleanup + Disposed + exact-unlink barrier.
    pub(crate) async fn dispose(self: &Arc<Self>) {
        if self.dispose_complete.load(Ordering::SeqCst) {
            return;
        }

        // Cancellation while waiting here is precommit and therefore inert.
        self.slot.claim().await;
        if self.dispose_complete.load(Ordering::SeqCst) {
            self.slot.abandon();
            return;
        }

        if self.disposing.swap(true, Ordering::SeqCst) {
            // The committed owner may have released the settle slot before its
            // exact Registry unlink completed. Give the slot back, then join
            // the owner's stronger terminal barrier rather than returning early.
            self.slot.abandon();
            self.wait_dispose_complete().await;
            return;
        }

        // Open-to-Closing is committed. No await occurs between the claim and
        // detaching the owner, so caller cancellation can abandon only the wait.
        let fiber = self.clone();
        crate::effect::detach(async move {
            fiber.complete_claimed_dispose().await;
        });
        self.wait_dispose_complete().await;
    }

    /// Finish a terminal disposal after the caller has won `disposing` and
    /// owns the lifecycle slot. This is the single implementation of the full
    /// terminal barrier shared by ordinary disposal and era replacement.
    pub(crate) async fn complete_claimed_dispose(self: &Arc<Self>) {
        settle_ctx::bracket(self.clone(), async {
            self.teardown_body().await;
            self.slot.abandon();
        })
        .await;
        // Completion includes exact residency release/prune; publish only after
        // the Registry barrier is complete.
        self.release_residency();
        self.publish_dispose_complete();
    }

    async fn wait_dispose_complete(&self) {
        loop {
            let notified = self.dispose_done.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.dispose_complete.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }

    fn publish_dispose_complete(&self) {
        self.dispose_complete.store(true, Ordering::SeqCst);
        self.dispose_done.notify_waiters();
    }

    /// The shared teardown body: drain every committed cleanup LIFO,
    /// withdraw the publications, mark the fiber dead, and unregister
    /// its dependency edges. Runs inside the settle bracket with the
    /// inertia slot owned by the caller — `dispose` claimed it, the
    /// initial pass holds it throughout, and the creation-cancellation
    /// guard inherits or claims it; the caller releases the slot
    /// afterwards.
    ///
    /// Idempotent against partial progress: an interrupted teardown
    /// (caller cancellation landing mid-drain) is completed by the
    /// creation guard's re-run — the journal claims each occurrence
    /// before running it, the death mark and edge unregister are
    /// one-synchronous-step, and a completed teardown (Disposed) is
    /// detected at the top so no transition regresses.
    async fn teardown_body(self: &Arc<Self>) {
        // the creation window closes with the teardown, whatever ended it
        self.creation_pending.store(false, Ordering::SeqCst);
        if self.state() == FiberState::Disposed {
            return; // a prior teardown ran to completion
        }
        self.transition(FiberState::Unloading).await;
        self.slot.pin_inactive();
        // mark dead BEFORE draining: every registration path (provide,
        // listeners, effects, spawn) gates on is_alive(), so this closes
        // the dispose-window TOCTOU — work pushed while cleanups run
        // fails fast with InactiveEffect instead of stranding slots and
        // hooks after the drain has already passed.
        //
        // The fiber's declared edges leave the index with its death mark
        // (ADR 0016): a
        // provide landing later in the drain window finds no entry, one
        // wasted kick fewer than the scan this replaced, which found the
        // fiber and let kick's alive gate absorb it. The root handle is
        // copied out under the spawn-state lock and the unregister runs
        // after its release — no nested acquisition (ADR 0010 decision 4).
        self.alive.store(false, Ordering::Relaxed);
        let root = self.spawn_state.root();
        if let Some(root) = root {
            root.deps.unregister(self);
        }
        self.drain_disposables().await;
        self.transition(FiberState::Disposed).await;
        self.store_error(None);
    }

    /// The creation-failure teardown: the caller owns the inertia slot
    /// (the initial pass holds it), the fiber's generation already ran
    /// its rollback inside the failed settle, and this completes the
    /// death mark, edge unregister, and exact residency release — the
    /// "no resident attempted Fiber" half of the failed-creation contract.
    /// The caller releases the slot afterwards.
    pub(crate) async fn creation_teardown(self: &Arc<Self>) {
        self.disposing.store(true, Ordering::SeqCst);
        settle_ctx::bracket(self.clone(), self.teardown_body()).await;
        self.force_unlink();
        self.publish_dispose_complete();
    }

    /// Completion of a caller-cancelled creation, driven by the spawn
    /// transaction's creation guard. The cancelled pass may have died
    /// holding the inertia slot — nobody else will release it — so slot
    /// ownership is *inherited* when the pass's ownership mark says so,
    /// and claimed normally otherwise (a kicked convergence can only be
    /// holding it transiently).
    pub(crate) async fn creation_interrupted_teardown(self: &Arc<Self>) {
        if self.slot.take_creation_ownership() {
            // the cancelled pass died holding the slot: inherit its
            // ownership and complete the teardown
            self.disposing.store(true, Ordering::SeqCst);
        } else {
            // the pass never owned the slot or released it before dying:
            // arbitrate against a concurrent dispose, then claim
            if self.disposing.swap(true, Ordering::SeqCst) {
                // A terminal disposer already committed. Creation cleanup must
                // coalesce onto that same full barrier rather than report its
                // attempted Fiber gone before exact unlink/prune completes.
                self.wait_dispose_complete().await;
                return;
            }
            self.slot.claim().await;
            if !self.is_alive() {
                self.slot.abandon();
                return;
            }
        }
        settle_ctx::bracket(self.clone(), self.teardown_body()).await;
        self.slot.abandon();
        self.force_unlink();
        self.publish_dispose_complete();
    }

    /// Release this Fiber's exact Registry residency claim, idempotently.
    /// A disposal racing creation may have completed the Fiber teardown but
    /// not yet released residency when the creation path reports, so creation
    /// endings force the exact unlink themselves. The claim prunes only its
    /// still-current empty allocation.
    pub(crate) fn force_unlink(self: &Arc<Self>) {
        self.release_residency();
    }

    pub(crate) fn release_residency(&self) {
        let claim = self.residency.lock().take();
        if let Some(claim) = claim {
            let root = self.spawn_state.root();
            let snapshot = root
                .as_ref()
                .map(|root| crate::observation::fiber_snapshot(root, self));
            claim.release();
            if let (Some(root), Some(fiber)) = (root, snapshot) {
                root.observations
                    .publish(crate::observation::RuntimeObservation::FiberResidency {
                        change: crate::observation::ResidencyChange::Removed,
                        fiber,
                    });
            }
        }
    }

    /// Claim a previously registered cleanup obligation without running
    /// it; returns the cleanup, if the occurrence was still
    /// generation-owned. The caller drops or runs it *after* any lock it
    /// holds has released (ADR 0010 two-phase).
    pub(crate) fn remove_disposable(
        &self,
        token: crate::effect::DisposableToken,
    ) -> Option<Cleanup> {
        self.disposables.lock().remove(token)
    }
}

pub(crate) fn refuse_group_removal_recursion(
    fibers: &[Arc<Fiber>],
) -> std::result::Result<(), LifecycleRecursion> {
    for fiber in fibers {
        settle_ctx::refuse_recursion(fiber, LifecycleOperation::RemovePlugins)?;
    }
    Ok(())
}

impl Context {
    /// Run a task bound to this context's fiber generation — the
    /// task-specific cleanup composition (ADR 0009 decision 5's W6
    /// home): a cooperative task the generation owns, drains, and
    /// joins.
    ///
    /// Registration commits into the generation **before** the task
    /// starts. Once committed, the task stays generation-owned
    /// regardless of what the caller does next — `run` is synchronous,
    /// so caller cancellation can only ever arrive after the commit it
    /// can no longer undo. When the generation closes (unload,
    /// restart/update replacement, failed-apply rollback, disposal),
    /// the drain runs every cleanup the task itself registered **first**
    /// (they were all pushed after the join-effect, so LIFO serves them
    /// first), and only then joins the task: the task observes its own
    /// resources tearing down and returns, and the join observes
    /// completion. A task's failure or panic is contained at that join
    /// and reported through the fiber's logger, never propagated into
    /// the drain.
    ///
    /// The task's output is consumed, never retained: the framework
    /// drops it inside the task as soon as the future resolves, so the
    /// output carries no `Send` bound (only the task future itself
    /// crosses to the executor). Neither the task's polling nor the
    /// output's destruction happens under a framework lock.
    ///
    /// # The ordering trick
    ///
    /// The join-effect is pushed **before** the task's own effects can be
    /// registered, so in a LIFO drain it runs *after* them: the
    /// task's stream-teardown effects run first, the task observes its
    /// inputs ending and returns, and only then does the drain join it.
    /// Pushing the join-effect after spawning instead races the task's
    /// own registrations — the task could land later effects that then
    /// run *after* the join, deadlocking the drain against a task
    /// whose inputs never closed. The [`JoinHandle`] parks in a cell
    /// because it does not exist at push time; the cleanup waits out
    /// that window, and the join itself runs inside the panic-containment
    /// boundary (ADR 0004) — a panicking task at drain is contained and
    /// reported, never propagated.
    ///
    /// # Errors
    ///
    /// [`TaskRegistrationError::InactiveContext`] when the fiber's
    /// generation is closed (stably Pending or Failed, draining, or
    /// disposed); [`TaskRegistrationError::ExecutorUnavailable`] when no
    /// async runtime is current to drive the task. Either refusal is
    /// pre-commit: nothing is registered and no task is started.
    ///
    /// [`JoinHandle`]: tokio::task::JoinHandle
    pub fn run<F>(&self, task: F) -> std::result::Result<(), TaskRegistrationError>
    where
        F: Future + Send + 'static,
    {
        let fiber = self.fiber().clone();
        fiber
            .assert_can_register()
            .map_err(|_| TaskRegistrationError::InactiveContext)?;
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|_| TaskRegistrationError::ExecutorUnavailable)?;
        // the parking cell: Pending until the spawn lands the handle,
        // Running while the task lives, Done once joined
        let cell: Arc<Mutex<RunSlot>> = Arc::new(Mutex::new(RunSlot::Pending));
        let parked = Arc::new(tokio::sync::Notify::new());
        let (join_cell, join_parked) = (cell.clone(), parked.clone());
        // the reporting channel for a panicking task at drain:
        // resolved at registration — the same fiber drains, so the name
        // is the one the task ran under
        let report_logger = self.logger();
        // The cleanup is a one-shot `FnOnce`: captures move straight into
        // the join loop, no re-clone per invocation.
        let cleanup: Cleanup = crate::effect::fut_cleanup(move || {
            Box::pin(async move {
                loop {
                    // enable-before-check: the spawn's park+notify can
                    // never be missed by a cleanup already waiting
                    let notified = join_parked.notified();
                    tokio::pin!(notified);
                    notified.as_mut().enable();
                    let joined = {
                        let mut slot = join_cell.lock();
                        match std::mem::replace(&mut *slot, RunSlot::Done) {
                            RunSlot::Running(join) => Some(join),
                            RunSlot::Pending => {
                                // not spawned yet: restore and wait for
                                // the park (run() reaches it without any
                                // await in between)
                                *slot = RunSlot::Pending;
                                None
                            }
                            RunSlot::Done => None,
                        }
                    };
                    if let Some(join) = joined {
                        crate::contained::contain_join("ctx.run task", Some(&report_logger), join)
                            .await;
                        return;
                    }
                    notified.await;
                }
            }) as crate::effect::BoxFuture<()>
        });
        // join-effect first (see the ordering trick above); the gated
        // seam's only refusal here is admission, so map it onto the
        // operation's variant
        crate::gated::push_gated(&fiber, cleanup, &mut crate::gated::NoPublish)
            .map_err(|_| TaskRegistrationError::InactiveContext)?;

        // then spawn and park — no await in between, so a concurrent
        // drain either sees Pending (waits for this park) or Running.
        // The wrapper consumes the task's output inside the task, so the
        // output needs no Send bound and the framework retains nothing:
        // the drain-join observes completion, not a value (callers
        // deliver outcomes through their own channels). The
        // settle-context scope marks this task as the fiber's own for
        // its whole life, so the fiber's lifecycle entries refuse from
        // inside it instead of deadlocking against the join this
        // registration created (ADR 0019).
        let scoped = fiber.clone();
        let join: tokio::task::JoinHandle<()> = handle.spawn(async move {
            settle_ctx::run_task(scoped, task).await;
        });
        *cell.lock() = RunSlot::Running(join);
        parked.notify_waiters();
        Ok(())
    }
}

/// Where a [`Context::run`] task's [`JoinHandle`](tokio::task::JoinHandle)
/// parks between spawn and drain-join.
enum RunSlot {
    /// Effect pushed, task not yet spawned.
    Pending,
    /// Task spawned; the cleanup joins this handle.
    Running(tokio::task::JoinHandle<()>),
    /// Joined (or never spawned) — nothing left to wait for.
    Done,
}

/// Public handle of a running plugin instance (mirrors `Fiber &
/// PromiseLike<Fiber>`; the promise mixin is JS-only — the port's
/// public-handle/private-Fiber split).
///
/// Cheap to clone; lifecycle operations go through the shared private Fiber
/// body. Correlation identity is exposed separately as an opaque [`FiberId`].
#[derive(Clone)]
pub struct FiberHandle {
    pub(crate) fiber: Arc<Fiber>,
}

impl fmt::Debug for FiberHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FiberHandle")
            .field("id", self.fiber.id())
            .field("name", &self.fiber.name)
            .finish()
    }
}

/// Lifecycle operation named by a typed recursion refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LifecycleOperation {
    /// Drive or await current-target convergence.
    Ready,
    /// Passively await one lifecycle-state publication.
    WaitState,
    /// Re-run the current Fiber generation.
    Restart,
    /// Commit one same-Fiber prepared change.
    Update,
    /// Replace one Fiber era with a fresh era.
    EraSwap,
    /// End one Fiber terminally.
    Dispose,
    /// Remove one typed PluginGroup allocation.
    RemovePlugins,
}

/// A lifecycle self-wait that would deadlock on one concrete Fiber allocation.
///
/// The value exposes semantic operation plus opaque correlation identity only;
/// allocation-address attribution remains private to the runtime.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{operation:?} cannot wait on {fiber_id:?} from that Fiber's settle context")]
pub struct LifecycleRecursion {
    operation: LifecycleOperation,
    fiber_id: FiberId,
}

impl LifecycleRecursion {
    pub(crate) fn new(operation: LifecycleOperation, fiber_id: FiberId) -> Self {
        Self {
            operation,
            fiber_id,
        }
    }

    /// The lifecycle operation that was refused.
    pub fn operation(&self) -> LifecycleOperation {
        self.operation
    }

    /// Correlation-only identity of the Fiber whose lifecycle wait was refused.
    pub fn fiber_id(&self) -> &FiberId {
        &self.fiber_id
    }
}

/// Why [`FiberHandle::ready`] could not report live quiescence.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ReadyError {
    /// The call would synchronously wait on its own settle context.
    #[error("{0}")]
    Recursion(LifecycleRecursion),
    /// The current SemanticTarget is parked on a completed Plugin apply failure.
    #[error("plugin apply failed: {0}")]
    Apply(PluginFailure),
}

/// Why an explicit same-Fiber restart could not reach quiescence.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RestartError {
    /// The Fiber was already terminally closing/closed before this restart committed.
    #[error("fiber is closed")]
    Closed,
    /// The call would synchronously wait on its own settle context.
    #[error("{0}")]
    Recursion(LifecycleRecursion),
    /// The committed restart re-apply parked a normalized Plugin failure.
    #[error("plugin apply failed: {0}")]
    Apply(PluginFailure),
}

/// Why a [`FiberHandle::wait_state`](FiberHandle::wait_state) call ended early: the
/// deadline passed first, or the call was refused outright.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WaitStateError {
    /// The deadline passed before the Fiber published the requested state.
    #[error("wait_state timed out before the fiber published the target state")]
    Elapsed,
    /// The call would synchronously wait on its own settle context.
    #[error("{0}")]
    Recursion(LifecycleRecursion),
}

impl FiberHandle {
    pub(crate) fn new(fiber: Arc<Fiber>) -> Self {
        Self { fiber }
    }

    /// Current lifecycle state of the underlying fiber.
    pub fn state(&self) -> FiberState {
        self.fiber.state()
    }

    /// Opaque Runtime-local identity shared by all clones of this FiberHandle.
    pub fn id(&self) -> FiberId {
        self.fiber.id().clone()
    }

    /// Display name resolved from the plugin name.
    pub fn name(&self) -> &str {
        &self.fiber.name
    }

    /// Names of this Fiber's declared dependencies that are currently Missing.
    ///
    /// This is a per-FiberHandle diagnostic projection only; it exposes neither
    /// Registry topology nor lifecycle authority and is not target identity.
    ///
    /// Empty once the fiber is disposed (liveness early-exit; `spawn_state`
    /// itself is kept because dispose-time pruning still reads it).
    pub fn pending_missing(&self) -> Vec<String> {
        if !self.fiber.is_alive() || self.fiber.state() != FiberState::Pending {
            return Vec::new();
        }
        // Root is copied out under the spawn-state lock; authoritative
        // dependency edges live on the Fiber and Service reads happen after
        // that guard is released.
        match self.fiber.spawn_state.root() {
            Some(root) => self.fiber.missing_from(&root),
            None => Vec::new(),
        }
    }

    /// Drive or await convergence to the current live `SemanticTarget`.
    ///
    /// The returned state is always quiescent `Active`, stable `Pending`, or
    /// terminal `Disposed`. A current parked Plugin failure is returned as
    /// [`ReadyError::Apply`]. Transient lifecycle states are never returned.
    /// Durable off-runtime Service drift is actively driven here before the
    /// quiescence check, so a failure parked on an older target cannot leak as
    /// the answer for a newer committed target.
    pub async fn ready(&self) -> std::result::Result<FiberState, ReadyError> {
        settle_ctx::refuse_recursion(&self.fiber, LifecycleOperation::Ready)
            .map_err(ReadyError::Recursion)?;
        let fiber = &self.fiber;
        let state = loop {
            if fiber.slot.has_committed_recheck()
                && let Some(root) = fiber.spawn_state.root()
            {
                fiber.slot.kick(fiber, &root);
            }
            fiber.slot.wait_idle().await;
            let observed = fiber.state();
            if fiber.slot.is_idle() && !fiber.slot.has_committed_recheck() {
                break observed;
            }
        };
        match state {
            FiberState::Failed => {
                let failure = fiber
                    .error
                    .lock()
                    .clone()
                    .expect("Failed is published only with a parked PluginFailure");
                Err(ReadyError::Apply(spawn::clone_owned(&failure)))
            }
            FiberState::Active | FiberState::Pending | FiberState::Disposed => Ok(state),
            FiberState::Loading | FiberState::Unloading => {
                unreachable!("an idle Fiber cannot publish a transient lifecycle state")
            }
        }
    }

    /// Passively observe publication of `state`, or report
    /// [`WaitStateError::Elapsed`] when `timeout` wins.
    ///
    /// This operation never claims or kicks the settle slot. Its reliable
    /// signal is the Fiber's dedicated state-publication revision: the notify
    /// is only a wake hint, so a requested state published and superseded
    /// before this task is polled again is still observed.
    pub async fn wait_state(
        &self,
        state: FiberState,
        timeout: Duration,
    ) -> std::result::Result<(), WaitStateError> {
        settle_ctx::refuse_recursion(&self.fiber, LifecycleOperation::WaitState)
            .map_err(WaitStateError::Recursion)?;

        self.fiber.state.wait_for(state, timeout).await
    }

    /// Terminally dispose this Fiber and await its complete barrier.
    ///
    /// The first Open-to-Closing claim owns cleanup, `Disposed` publication,
    /// exact Registry unlink, and conditional prune under framework ownership.
    /// Concurrent/later calls coalesce onto that same completion; completed
    /// repeats succeed without replaying cleanup or lifecycle observation.
    /// Caller cancellation after the terminal claim abandons only that caller's
    /// wait and cannot stop the owner transaction.
    ///
    /// A self-wait from this Fiber's settle context is refused before the
    /// terminal claim with [`LifecycleRecursion`] naming
    /// [`LifecycleOperation::Dispose`].
    pub async fn dispose(&self) -> std::result::Result<(), LifecycleRecursion> {
        settle_ctx::refuse_recursion(&self.fiber, LifecycleOperation::Dispose)?;
        self.fiber.dispose().await;
        Ok(())
    }

    /// Restart this same Fiber/FiberId against its current committed input
    /// and immutable dependency edges.
    ///
    /// Admission serializes behind in-flight lifecycle work. Once generation
    /// replacement commits, the old generation is closed and drained and the
    /// restart completes under framework ownership despite caller cancellation.
    /// The operation retains lifecycle ownership through release-time target
    /// drift and returns only after the latest target is quiescent `Active` or
    /// stable `Pending`, or after parking [`RestartError::Apply`]. Explicit
    /// restart retries even a failure parked on the same target.
    ///
    /// A doomed self-wait is refused as [`RestartError::Recursion`] before
    /// terminal admission or any lifecycle effect. Otherwise a Fiber that is
    /// already terminal returns [`RestartError::Closed`] without change.
    pub async fn restart(&self) -> std::result::Result<(), RestartError> {
        settle_ctx::refuse_recursion(&self.fiber, LifecycleOperation::Restart)
            .map_err(RestartError::Recursion)?;
        if !self.fiber.is_alive() {
            return Err(RestartError::Closed);
        }
        self.fiber.slot.restart_pass(&self.fiber).await
    }

    /// Consume one typed prepared input replacement through precommit control and, if
    /// accepted, replace this Fiber's generation without changing its identity.
    ///
    /// A doomed self-wait is refused as [`UpdateError::Recursion`] before liveness,
    /// contract checks, update control, or any lifecycle effect. Contract mismatch,
    /// veto, control failure, or lost admission commit nothing. Once lifecycle commit
    /// occurs, the final transformed candidate is authoritative;
    /// postcommit apply failure is reported through [`UpdateError::Apply`] and cannot
    /// be recovered by update policy. Use [`Self::era_swap`] when replacement must
    /// intentionally break Fiber identity.
    pub async fn update(
        &self,
        change: crate::PreparedChange,
    ) -> std::result::Result<UpdateOutcome, UpdateError> {
        settle_ctx::refuse_recursion(&self.fiber, LifecycleOperation::Update)
            .map_err(UpdateError::Recursion)?;
        if !self.fiber.is_alive() {
            return Err(UpdateError::Closed);
        }
        if self.fiber.spawn_state.contract() != Some(change.contract()) {
            return Err(UpdateError::PluginContractMismatch);
        }
        let ctx = self
            .fiber
            .spawn_state
            .required_context_for(&self.fiber)
            .map_err(|_| UpdateError::Closed)?;
        let accepted = ctx
            .root
            .updates
            .control(change.contract(), ctx.scope.clone(), change)
            .await
            .map_err(UpdateError::Control)?;
        let Some(change) = accepted else {
            return Ok(UpdateOutcome::Vetoed);
        };
        let state = self.fiber.slot.update_pass(&self.fiber, change).await?;
        debug_assert!(matches!(state, FiberState::Active | FiberState::Pending));
        Ok(UpdateOutcome::Committed(state))
    }
}
