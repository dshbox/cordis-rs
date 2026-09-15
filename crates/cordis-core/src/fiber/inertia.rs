//! [`InertiaSlot`] — the settle protocol's single home (ADR 0004): the
//! convergence state machine, the SemanticTarget cell, the waiter
//! wakeups, and the lifecycle passes that compose the ops — initial
//! spawn, convergence, restart. Everything about *when* a fiber may
//! settle, what target it settles toward, who owes a wakeup, and in what
//! order the ops run lives here; nothing outside this module touches its
//! fields.

use crate::context::Root;
use crate::fiber::spawn_state::EffectiveApplyInput;
use crate::fiber::{DependencyEdge, Fiber, RestartError};
use crate::service::ServiceOccurrenceId;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The exact semantic value a Fiber settles against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SemanticTarget {
    input: EffectiveApplyInput,
    assignments: Vec<(DependencyEdge, Option<ServiceOccurrenceId>)>,
}

impl SemanticTarget {
    pub(super) fn new(
        input: EffectiveApplyInput,
        assignments: Vec<(DependencyEdge, Option<ServiceOccurrenceId>)>,
    ) -> Self {
        Self { input, assignments }
    }

    pub(crate) fn has_missing(&self) -> bool {
        self.assignments
            .iter()
            .any(|(_, publication)| publication.is_none())
    }
}

/// No pass in flight or pending — the only quiescent value.
const INERTIA_IDLE: u64 = 0;
/// A settle/converge pass holds the slot.
const INERTIA_ACTIVE: u64 = 1;
/// The holder's pass finished; drift recheck still pending.
const INERTIA_RELEASING: u64 = 2;

/// The settle protocol's one home: the fiber's convergence state machine.
///
/// Three pieces of state, every rule for using them:
///
/// - `inertia: AtomicU64` (`SeqCst` throughout) — IDLE / ACTIVE /
///   RELEASING. `IDLE` is the *only* quiescent value: [`FiberHandle::ready`](crate::FiberHandle::ready)
///   and [`Self::claim`] treat it as "no pass in flight or pending", so a
///   holder may only store it once it has verified no follow-up pass will
///   start. `RELEASING` is the recheck state that makes that verifiable:
///   the holder finished its settle pass but has not yet confirmed the
///   semantic target stayed put. Releasing *through* `IDLE` (store 0,
///   re-check, CAS back to 1) left a window where `ready()` observed
///   quiescence while a follow-up convergence was about to start —
///   callers could act on stale dependency state (v1 sixth-pass #9, the
///   carried fix).
/// - `target: Mutex<Option<SemanticTarget>>` — the last attempted/settled
///   semantic target. Each target combines committed effective input with every
///   Fiber-owned exact slot assigned to Missing or one exact publication occurrence.
/// - `done: Notify` — the wakeup channel. **Every** store of `IDLE` owes a
///   `notify_waiters`: waiters register interest *before* re-checking the
///   slot (`Notified::enable`), the lost-wakeup protection both waiting
///   sides rely on.
///
/// # Who may write the target cell, and when
///
/// Writes pair with slot ownership in exactly three shapes:
///
/// Service mutations first advance `recheck_committed`; that durable revision
/// is protocol truth and survives missing executors. A kick only tries to hand
/// the outstanding revision to a driver. If it races `RELEASING`, it changes
/// that state back to `ACTIVE`, so the releasing holder's final CAS to `IDLE`
/// fails and the holder keeps ownership. If it lands after `IDLE`, it claims
/// the slot and starts one convergence pass. Thus no mutation can disappear in
/// the recheck-to-idle window, and no two drivers may apply concurrently.
///
/// Stale pre-written semantic targets are deliberately NOT normalized after a
/// clean recheck: each stored publication-occurrence identity clone keeps its
/// identity allocation alive, so a replacement publication cannot compare
/// equal to stale residue — the cell can neither mask a future drift nor invent one.
///
/// A failed apply deliberately keeps the target at the failed semantic target
/// (no Missing-sentinel reset — ADR 0003 §2.2 item 5, a ratified divergence from
/// upstream's fiber.ts:421-426): with the reset, every later
/// semantic target-preserving trigger would re-detect "drift" and retry the
/// failed apply, which upstream does not do. Dispose pins
/// the target cell absent through [`Self::pin_inactive`] — a dead fiber never converges
/// again.
pub(crate) struct InertiaSlot {
    /// Convergence state machine word; see the struct docs.
    inertia: AtomicU64,
    /// Dependency semantic target cell; see the struct docs.
    target: parking_lot::Mutex<Option<SemanticTarget>>,
    /// Monotonic durable Service-drift commit sequence. Visibility mutations
    /// advance this before any best-effort kick; executor availability cannot
    /// consume or erase the obligation.
    recheck_committed: AtomicU64,
    /// Last recheck revision proven clean at a release boundary.
    recheck_settled: AtomicU64,
    /// Wakes `ready()`/`claim()` waiters; every IDLE store owes a notify.
    done: tokio::sync::Notify,
    /// The initial spawn pass's ownership mark, set while the pass holds
    /// the slot and cleared before it releases. The spawn transaction's
    /// creation guard reads it to tell a *heirless* slot (the cancelled
    /// pass died holding it — ownership must be inherited, never claimed,
    /// since no release is coming) from a merely busy one (a kicked
    /// convergence the dead pass was waiting on — claim and wait it out).
    creation_owned: std::sync::atomic::AtomicBool,
}

/// How the initial spawn pass ended — the spawn transaction's mapping
/// input. The pass returns only once the creation reached a complete
/// ending: the slot is always released (`IDLE`) first.
pub(crate) enum InitialOutcome {
    /// The fiber settled `Active` and stayed settled through the release
    /// recheck — live quiescent.
    Active,
    /// Required services are missing: the fiber is stably `Pending`,
    /// no apply ran, and a later publication kicks the convergence.
    Pending,
    /// The initial apply returned an error or panicked. The pass
    /// completed the rollback, teardown, and unlink; the normalized
    /// failure rides along for [`SpawnError::InitialApply`](super::SpawnError).
    Failed(super::PluginFailure),
    /// Framework invalidation disposed the attempted fiber before
    /// handoff; the pass made the disposal and unlink deterministic.
    Interrupted,
}

impl InertiaSlot {
    pub(crate) fn new() -> Self {
        Self {
            inertia: AtomicU64::new(INERTIA_IDLE),
            target: parking_lot::Mutex::new(None),
            recheck_committed: AtomicU64::new(0),
            recheck_settled: AtomicU64::new(0),
            done: tokio::sync::Notify::new(),
            creation_owned: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// The initial pass marks its slot ownership for the creation guard
    /// (see the field docs). Synchronous with the claim it follows: the
    /// spawn future can only be dropped at an await, so the mark is
    /// always accurate when the guard observes it.
    fn set_creation_owned(&self, owned: bool) {
        self.creation_owned.store(owned, Ordering::SeqCst);
    }

    /// The creation guard's probe: `true` (once) when the cancelled pass
    /// died holding the slot — the caller inherits that ownership and
    /// owes the release. `false` means claim normally.
    pub(crate) fn take_creation_ownership(&self) -> bool {
        self.creation_owned.swap(false, Ordering::SeqCst)
    }

    /// The current settle target (the stored semantic target).
    fn current_target(&self) -> SemanticTarget {
        self.target
            .lock()
            .clone()
            .expect("an active convergence pass has a recorded SemanticTarget")
    }

    /// Holder records the semantic target it settled toward — the SETTLED
    /// target, never a freshly computed live semantic target: a service change
    /// that raced the apply must stay visible as drift to the release
    /// recheck.
    fn record_target(&self, target: SemanticTarget) {
        *self.target.lock() = Some(target);
    }

    /// Dispose clears the target cell: the target is read-only from here on — a
    /// disposed fiber must never converge again, its plugin will not be
    /// re-applied.
    pub(crate) fn pin_inactive(&self) {
        *self.target.lock() = None;
    }

    /// The spawn path's guarded Pending write: record the exact Missing target
    /// only if the cell still holds the untouched target cell. A
    /// provide racing the spawn window may already have stored the live
    /// semantic target — clobbering it with a stale Missing target would send an
    /// in-flight converge loop chasing a stale target for one extra
    /// unload/apply pass (it self-corrects, but why churn).
    fn mark_pending_if_untouched(&self, pending: &SemanticTarget) {
        let mut stored = self.target.lock();
        if stored.is_none() {
            *stored = Some(pending.clone());
        }
    }

    /// Wait until the slot is free, then claim it (IDLE → ACTIVE, CAS so
    /// two claimers can never both win). Wakeup interest is registered
    /// BEFORE re-checking the slot (`Notified::enable`) so a release
    /// racing the check can never be missed — the consumer-side half of
    /// the lost-wakeup protection (the producer side is the release path
    /// through [`Self::exit_recheck`]).
    pub(crate) async fn claim(&self) {
        loop {
            let notified = self.done.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self
                .inertia
                .compare_exchange(
                    INERTIA_IDLE,
                    INERTIA_ACTIVE,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
            {
                return;
            }
            notified.await;
        }
    }

    /// Commit one durable target-recheck obligation. This is protocol truth:
    /// callers do it before attempting any executor-dependent kick.
    pub(crate) fn commit_recheck(&self) {
        self.recheck_committed.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn has_committed_recheck(&self) -> bool {
        self.recheck_committed.load(Ordering::SeqCst) != self.recheck_settled.load(Ordering::SeqCst)
    }

    fn recheck_revision(&self) -> u64 {
        self.recheck_committed.load(Ordering::SeqCst)
    }

    /// Wait for quiescence: the observe side behind
    /// [`FiberHandle::ready`](crate::FiberHandle::ready). Enable-before-check, same
    /// lost-wakeup discipline as [`Self::claim`]; returns only once the
    /// slot reads IDLE, which a holder may store only after a clean drift
    /// recheck — so returning implies the fiber converged for the current
    /// service snapshot.
    pub(crate) async fn wait_idle(&self) {
        loop {
            let notified = self.done.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.inertia.load(Ordering::SeqCst) == INERTIA_IDLE {
                return;
            }
            notified.await;
        }
    }

    /// Holder gives the slot back **without** a drift recheck. Only for
    /// holders that cannot or need not converge: dead/terminal disposal paths,
    /// initial-spawn endings that already performed their own live-target loop,
    /// and direct test/protocol recovery paths. Ordinary convergence and
    /// committed restart release through guarded [`Self::exit_recheck`].
    pub(crate) fn abandon(&self) {
        self.inertia.store(INERTIA_IDLE, Ordering::SeqCst);
        self.done.notify_waiters();
    }

    /// Whether the slot currently reads IDLE — the quiescence probe
    /// [`FiberHandle::ready`](crate::FiberHandle::ready) uses to reject states observed
    /// after a pass snuck between its wake and its state read.
    pub(crate) fn is_idle(&self) -> bool {
        self.inertia.load(Ordering::SeqCst) == INERTIA_IDLE
    }

    /// One-shot claim attempt (IDLE → ACTIVE CAS) for holders that race a
    /// kicked convergence loop — [`Fiber::initial_settle`](super::Fiber)
    /// must not silently co-exist with an in-flight pass.
    fn try_claim(&self) -> bool {
        self.inertia
            .compare_exchange(
                INERTIA_IDLE,
                INERTIA_ACTIVE,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
    }

    /// The convergence loop's exit recheck: judge drift from RELEASING,
    /// never through IDLE. Returns `true` on drift (the loop continues
    /// toward the recorded new target, still holding the slot); on a clean
    /// probe the slot is abandoned and `false` returns — a `ready()` that
    /// observes the resulting idle may conclude the fiber converged.
    fn exit_recheck(
        &self,
        fiber: &Arc<Fiber>,
        root: &Arc<Root>,
        settled: &SemanticTarget,
        mut revision: u64,
    ) -> bool {
        loop {
            self.inertia.store(INERTIA_RELEASING, Ordering::SeqCst);
            let live = fiber.compute_target(root);
            let observed_revision = self.recheck_revision();
            if live != *settled {
                self.record_target(live);
                self.inertia.store(INERTIA_ACTIVE, Ordering::SeqCst);
                return true;
            }
            if observed_revision != revision {
                // Revisions are durable wakeup/progress authority, not part of
                // SemanticTarget equality (ADR 0031). Re-probe after every
                // revision observed during this pass so a target-neutral
                // notification is acknowledged without re-applying, while a
                // semantic mutation cannot hide behind a stale target read.
                revision = observed_revision;
                continue;
            }

            self.recheck_settled.store(revision, Ordering::SeqCst);
            match self.inertia.compare_exchange(
                INERTIA_RELEASING,
                INERTIA_IDLE,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => {
                    self.done.notify_waiters();
                    return false;
                }
                Err(INERTIA_ACTIVE) => {
                    // A committed recheck claimed our releasing slot. It is a
                    // signal to probe again, not proof of semantic drift; the
                    // existing holder still owns the driver.
                    revision = self.recheck_revision();
                }
                Err(other) => panic!("invalid inertia release state {other}"),
            }
        }
    }

    /// The synchronous trigger entry for an effective Service visibility
    /// mutation: detect semantic target drift and ensure exactly one
    /// pass converges toward it — kick a new converge task, or trust the
    /// in-flight holder to pick the mutation up.
    ///
    /// Called only after the durable recheck revision has been committed.
    /// Off-runtime IDLE kicks simply leave that revision outstanding; an
    /// off-runtime kick racing RELEASING still claims the slot back for the
    /// existing holder, requiring no spawned task.
    pub(crate) fn kick(&self, fiber: &Arc<Fiber>, root: &Arc<Root>) {
        if !fiber.is_alive() || !self.has_committed_recheck() {
            return;
        }

        loop {
            match self.inertia.load(Ordering::SeqCst) {
                INERTIA_ACTIVE => return,
                INERTIA_RELEASING => {
                    if self
                        .inertia
                        .compare_exchange(
                            INERTIA_RELEASING,
                            INERTIA_ACTIVE,
                            Ordering::SeqCst,
                            Ordering::SeqCst,
                        )
                        .is_ok()
                    {
                        // This claim needs no executor: the releasing holder is
                        // already the driver. Its final CAS to IDLE now fails,
                        // so it keeps ownership and rechecks the latest target.
                        return;
                    }
                }
                INERTIA_IDLE => {
                    let Ok(handle) = tokio::runtime::Handle::try_current() else {
                        // The durable revision remains outstanding. A later
                        // ready/lifecycle driver can claim it without another
                        // mutation.
                        return;
                    };
                    if self
                        .inertia
                        .compare_exchange(
                            INERTIA_IDLE,
                            INERTIA_ACTIVE,
                            Ordering::SeqCst,
                            Ordering::SeqCst,
                        )
                        .is_ok()
                    {
                        let revision = self.recheck_revision();
                        let settled = self.target.lock().clone();
                        if let Some(settled) = settled {
                            if self.exit_recheck(fiber, root, &settled, revision) {
                                drop(Self::spawn_convergence_pass(&handle, fiber, root));
                            }
                        } else {
                            // Before the creation pass records its first target,
                            // preserve the spawn-window choreography: the
                            // convergence winner owns the slot, but settle_once
                            // still leaves the first apply to creation.
                            self.record_target(fiber.compute_target(root));
                            drop(Self::spawn_convergence_pass(&handle, fiber, root));
                        }
                        return;
                    }
                }
                other => panic!("invalid inertia state {other}"),
            }
        }
    }

    /// Release the slot after an inline settle (spawn/restart) — or keep
    /// it, when the semantic target drifted meanwhile: control passes to a
    /// fresh convergence task instead of losing the update.
    ///
    /// `settled` is the semantic target the in-flight settle applied toward
    /// (a Missing assignment, e.g. a spawn that stayed Pending, is a legal settled
    /// value); see
    /// [`Self::drifted_since`] for why the comparison is not against the
    /// stored target.
    ///
    /// Hand the (already-ACTIVE) slot to a fresh convergence task after a
    /// best-effort kick wins an idle slot. The clones live here so the spawned
    /// future owns its Fiber and root; the pass borrows them for its whole life.
    fn spawn_convergence_pass(
        handle: &tokio::runtime::Handle,
        fiber: &Arc<Fiber>,
        root: &Arc<Root>,
    ) -> tokio::task::JoinHandle<()> {
        let logger = root.logger.logger_for_fiber(&fiber.name);
        let context = format!("fiber={:?}", fiber.name);
        let fiber = fiber.clone();
        let root = root.clone();
        crate::framework_task::spawn(handle, logger, "fiber convergence", context, async move {
            fiber.slot.convergence_pass(&fiber, &root).await
        })
    }

    /// Drive the initial settle pass of a freshly spawned fiber: claim the
    /// inertia slot (CAS, not a bare store), then settle toward the live
    /// semantic target and keep converging inline until the release recheck
    /// comes back clean — the spawn transaction delivers a FiberHandle only for
    /// a *live quiescent* fiber, so raced-in service mutations are
    /// converged here, never handed to a background pass the caller
    /// cannot see.
    ///
    /// The claim must be a CAS: by the time this runs, the dependency
    /// edge registration has already published the fiber to affected-set
    /// discovery, so a concurrent visibility commit in that window may have
    /// claimed the slot through the committed-recheck driver. A bare store
    /// would silently co-exist with that loop and run a second settle pass
    /// concurrently — two applies on one fiber (duplicated listeners, or
    /// a double provide turning the fiber Failed).
    ///
    /// On a lost claim the spawner waits for the slot and settles inline.
    /// The winner is either a convergence driver or the current holder kept
    /// active by a committed recheck; either way the first apply belongs to
    /// this pass: a convergence pass never runs the first apply of a
    /// creation-pending fiber (see `settle_once`'s `owns_first_apply`), so
    /// the loser neither double-applies nor adopts a first-apply failure
    /// that must surface as the creation's `InitialApply` ending.
    ///
    /// The endings, all with the slot released:
    ///
    /// - the fiber is dead before the pass starts (typed group removal
    ///   won the race and disposed it): [`InitialOutcome::Interrupted`],
    ///   with the exact residency release forced here so the attempted
    ///   Fiber is unlinked before the transaction reports it;
    /// - the live SemanticTarget has a Missing assignment: the fiber stays Pending
    ///   without running apply, the exact Missing target is recorded, and the
    ///   release recheck still runs — a provide that landed inside this
    ///   very window is drift and loops back to the settle;
    /// - the apply returned an error or panicked: the failed settle
    ///   already ran the generation's LIFO rollback, so the pass runs
    ///   the creation teardown (death mark, edge unregister, runtime
    ///   unlink) while still holding the slot — no kicked convergence
    ///   can re-apply a fiber whose creation failed — and answers
    ///   [`InitialOutcome::Failed`] with the normalized failure;
    /// - otherwise the fiber settled Active: [`InitialOutcome::Active`].
    ///
    /// While the pass holds the slot it carries the `creation_owned`
    /// mark, so the spawn transaction's creation guard can tell a slot
    /// the cancelled pass died holding (inherit, never claim) from a
    /// merely busy one.
    pub(crate) async fn initial_spawn_pass(
        &self,
        fiber: &Arc<Fiber>,
        root: &Arc<Root>,
    ) -> InitialOutcome {
        if !self.try_claim() {
            self.claim().await;
        }
        if !fiber.is_alive() {
            // dispose raced the spawn and won — on either claim path:
            // a completed dispose leaves the slot IDLE, so a winning
            // try_claim can also observe the death. The winner's own
            // unlink may lag its slot release by a few instructions;
            // force it (idempotent) so the reported interruption is
            // fully disposed *and* unlinked.
            fiber.force_unlink();
            self.abandon();
            return InitialOutcome::Interrupted;
        }
        self.set_creation_owned(true);
        let outcome = loop {
            let revision = self.recheck_revision();
            let target = fiber.compute_target(root);
            let inactive = target.has_missing();
            fiber.settle_once(&target, true).await;
            if inactive {
                // required services missing: stay Pending; provide()
                // will kick us. The Pending write is guarded so a
                // racing visibility commit is never acknowledged as settled, and
                // the recheck treats any complete live semantic target as
                // drift (the settled target has Missing here) and loops.
                self.mark_pending_if_untouched(&target);
                if self.exit_recheck(fiber, root, &target, revision) {
                    continue;
                }
                break InitialOutcome::Pending;
            }
            self.record_target(target.clone());
            if fiber.state() == crate::fiber::FiberState::Failed {
                let failure = fiber
                    .take_error()
                    .map(super::spawn::into_owned)
                    .unwrap_or_else(|| {
                        super::PluginFailure::returned(
                            "plugin failed without a parked failure".to_owned(),
                        )
                    });
                // the failed settle ran the generation's LIFO rollback;
                // complete the teardown under the still-held slot
                fiber.creation_teardown().await;
                break InitialOutcome::Failed(failure);
            }
            // changes racing the initial apply are converged inline:
            // the FiberHandle is delivered only for a live quiescent Fiber
            if self.exit_recheck(fiber, root, &target, revision) {
                continue;
            }
            break InitialOutcome::Active;
        };
        self.set_creation_owned(false);
        // Pending/Active exits already published IDLE through the guarded
        // release CAS. Failed/dead exits cannot drift and still owe a plain
        // release of the slot they hold.
        if !self.is_idle() {
            self.abandon();
        }
        outcome
    }

    /// The inertia convergence loop: unload/apply toward the current
    /// target, then **re-read** the live semantic target — only exit once it
    /// matches, so bursts of service changes eventually settle into
    /// exactly one final state (mirrors the `_reload`/`_unload` tails
    /// chaining into each other, fiber.ts:427-457). The exit's drift
    /// recheck runs inside RELEASING, never through IDLE: a
    /// committed recheck that lands against our still-held slot relies on
    /// the holder re-reading the semantic target, and `ready()` may
    /// only observe IDLE once that recheck came back clean.
    ///
    /// Module-private on purpose: its spawner is [`Self::kick`] — explicit
    /// lifecycle passes retain their own slot through their documented barrier.
    async fn convergence_pass(&self, fiber: &Arc<Fiber>, root: &Arc<Root>) {
        loop {
            let revision = self.recheck_revision();
            let target = self.current_target();
            fiber.settle_once(&target, false).await;
            let live = fiber.compute_target(root);
            if live != target {
                self.record_target(live);
                continue;
            }
            // tentative exit: recheck from RELEASING, never through IDLE.
            // The loop continues itself on drift; it already owns the
            // convergence slot and may not release through IDLE first.
            if self.exit_recheck(fiber, root, &target, revision) {
                continue; // still non-idle; loop re-settles toward the new target
            }
            return;
        }
    }

    /// Admit one explicit restart, commit generation replacement, then
    /// transfer the complete current-target transaction to framework-owned
    /// work. Cancellation while waiting for the slot is precommit and inert;
    /// cancellation after the synchronous replacement marker abandons only the
    /// caller's completion wait.
    pub(crate) async fn restart_pass(
        &self,
        fiber: &Arc<Fiber>,
    ) -> std::result::Result<(), RestartError> {
        // Cancellation while waiting for admission is precommit and inert.
        self.claim().await;
        if !fiber.is_alive() {
            self.abandon();
            return Err(RestartError::Closed);
        }

        // A live public FiberHandle always has installed spawn state. Missing state
        // here is a framework invariant violation, not a consumer-visible
        // restart phase with a fourth error meaning.
        let root = fiber
            .spawn_state
            .required_root()
            .expect("live non-root Fiber has installed spawn state");

        let revision = self.recheck_revision();
        let target = fiber.compute_target(&root);
        self.record_target(target.clone());
        // Irreversible generation-replacement commit. This synchronous marker
        // participates in gated publication's under-journal-lock recheck, so a
        // registration either commits before us and is drained, or loses and
        // publishes nothing. The detached owner clears it only when Loading
        // opens the replacement generation.
        fiber.generation_replacing.store(true, Ordering::SeqCst);

        // The restart transaction is now committed: the lifecycle slot and
        // target belong to this operation. Transfer all remaining awaits to a
        // framework-owned task before yielding again so caller cancellation can
        // abandon only its wait, never strand a half-closed generation.
        let (tx, rx) = tokio::sync::oneshot::channel();
        let fiber = fiber.clone();
        crate::effect::detach(async move {
            let result = fiber
                .slot
                .restart_committed(&fiber, &root, target, revision)
                .await;
            let _ = tx.send(result);
        });

        rx.await
            .expect("framework-owned restart transaction always publishes completion")
    }

    /// Complete one committed restart while retaining the lifecycle slot
    /// through every release-time drift recheck. Restart's barrier is current-
    /// target quiescence, so it may not hand raced drift to a background pass
    /// and return early.
    async fn restart_committed(
        &self,
        fiber: &Arc<Fiber>,
        root: &Arc<Root>,
        mut target: SemanticTarget,
        mut revision: u64,
    ) -> std::result::Result<(), RestartError> {
        loop {
            fiber.settle_once(&target, false).await;
            let result = match fiber.error.lock().clone() {
                Some(failure) => Err(RestartError::Apply(super::spawn::clone_owned(&failure))),
                None => Ok(()),
            };

            if !self.exit_recheck(fiber, root, &target, revision) {
                return result;
            }

            revision = self.recheck_revision();
            target = fiber.compute_target(root);
            self.record_target(target.clone());
        }
    }
    /// Revalidate update admission, commit the final typed candidate, and transfer
    /// every postcommit await to framework-owned work.
    pub(crate) async fn update_pass(
        &self,
        fiber: &Arc<Fiber>,
        change: crate::PreparedChange,
    ) -> std::result::Result<crate::fiber::FiberState, crate::fiber::UpdateError> {
        self.claim().await;
        if !fiber.is_alive() {
            self.abandon();
            return Err(crate::fiber::UpdateError::AdmissionLost);
        }
        let root = fiber
            .spawn_state
            .required_root()
            .expect("live Fiber has spawn state");
        // Replacement admission closes synchronously at the lifecycle commit point,
        // before the authoritative Plugin input changes. A racing registration
        // therefore linearizes wholly before this replacement or observes the
        // closed gate and cannot escape the old-generation drain.
        fiber.generation_replacing.store(true, Ordering::SeqCst);
        fiber
            .spawn_state
            .commit_change(change)
            .expect("precommit contract check and slot ownership make commit infallible");
        let revision = self.recheck_revision();
        let target = fiber.compute_target(&root);
        self.record_target(target.clone());
        let (tx, rx) = tokio::sync::oneshot::channel();
        let fiber = fiber.clone();
        crate::effect::detach(async move {
            let result = fiber
                .slot
                .update_committed(&fiber, &root, target, revision)
                .await;
            let _ = tx.send(result);
        });
        rx.await
            .expect("framework-owned update transaction publishes completion")
    }

    async fn update_committed(
        &self,
        fiber: &Arc<Fiber>,
        root: &Arc<Root>,
        mut target: SemanticTarget,
        mut revision: u64,
    ) -> std::result::Result<crate::fiber::FiberState, crate::fiber::UpdateError> {
        loop {
            fiber.settle_once(&target, false).await;
            let result = match fiber.error.lock().clone() {
                Some(failure) => Err(crate::fiber::UpdateError::Apply(super::spawn::clone_owned(
                    &failure,
                ))),
                None => Ok(fiber.state()),
            };
            if !self.exit_recheck(fiber, root, &target, revision) {
                return result;
            }
            revision = self.recheck_revision();
            target = fiber.compute_target(root);
            self.record_target(target.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::context::Context;
    use crate::context::RealmKey;
    use crate::deadline::bounded;
    use crate::fiber::{Fiber, FiberState};
    use crate::logger::{BufferExporter, Level};
    use std::sync::Arc;

    /// The direct test tier for the settle protocol — each invariant below
    /// corresponds to a class of race fix the protocol already survived
    /// (contract tests pin the same behavior end-to-end; these pin the
    /// slot's own interface). The one arm that cannot be made
    /// deterministic at unit level — a kick whose CAS wins off-runtime —
    /// stays with the contract tier (the off-runtime service tests,
    /// core-v2 18/21).
    fn fiber_requiring(ctx: &Context, inject: &[&str]) -> Arc<Fiber> {
        let edges = inject
            .iter()
            .map(|name| crate::fiber::DependencyEdge::new((*name).to_string(), RealmKey::DEFAULT))
            .collect();
        let fiber = Fiber::new_with_edges("test", edges);
        let prepared = crate::PreparedPlugin::from_input(
            CountApplies {
                applies: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                fail: false,
            },
            (),
        );
        let crate::PreparedPlugin {
            plugin,
            name,
            inject,
            contract,
        } = prepared;
        fiber
            .spawn_state
            .install(
                plugin,
                name,
                inject,
                ctx.clone(),
                crate::registry::PluginKey::Typed(contract),
                ctx.clone(),
            )
            .unwrap();
        ctx.root.deps.register(&fiber);
        fiber
    }

    /// A plugin-counting probe with an installed spawn state, inside the
    /// creation window — the shape a fiber has between the spawn's
    /// dependency-edge publication and its initial pass.

    #[derive(Debug, thiserror::Error)]
    #[error("test apply failure")]
    struct TestApplyError;

    struct CountApplies {
        applies: Arc<std::sync::atomic::AtomicUsize>,
        fail: bool,
    }

    impl crate::Plugin for CountApplies {
        type Config = ();
        type Input = ();
        type PrepareError = std::convert::Infallible;
        type ApplyError = TestApplyError;

        fn prepare(&self, _config: ()) -> std::result::Result<(), std::convert::Infallible> {
            Ok(())
        }

        async fn apply(
            &self,
            _ctx: Context,
            _prepared: &(),
        ) -> std::result::Result<(), TestApplyError> {
            self.applies
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.fail {
                return Err(TestApplyError);
            }
            Ok(())
        }
    }

    fn fiber_in_creation(
        ctx: &Context,
        applies: Arc<std::sync::atomic::AtomicUsize>,
        fail: bool,
    ) -> Arc<Fiber> {
        let fiber = Fiber::new("test");
        fiber
            .creation_pending
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let crate::PreparedPlugin {
            plugin,
            name,
            inject,
            contract,
        } = crate::PreparedPlugin::from_input(CountApplies { applies, fail }, ());
        fiber
            .spawn_state
            .install(
                plugin,
                name,
                inject,
                ctx.clone(),
                crate::registry::PluginKey::Typed(contract),
                ctx.clone(),
            )
            .unwrap();
        fiber
    }

    #[tokio::test]
    async fn a_convergence_winner_leaves_the_first_apply_to_the_initial_pass() {
        let ctx = Context::new();
        let root = ctx.root.clone();
        let applies = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let fiber = fiber_in_creation(&ctx, applies.clone(), false);

        // a kicked convergence wins the slot in the spawn window: it must
        // NOT run the first apply
        assert!(fiber.slot.try_claim());
        let target = fiber.compute_target(&root);
        fiber.settle_once(&target, false).await;
        assert_eq!(
            applies.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a convergence pass never runs a creation-pending first apply"
        );
        assert_eq!(fiber.state(), FiberState::Pending);
        fiber.slot.abandon();

        // the initial pass then runs it — exactly once
        let outcome = fiber.slot.initial_spawn_pass(&fiber, &root).await;
        assert!(matches!(outcome, super::InitialOutcome::Active));
        assert_eq!(
            applies.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the creation applied exactly once (LF-01's single transition)"
        );
    }

    #[tokio::test]
    async fn a_first_apply_failure_is_the_creations_initial_apply_ending() {
        let ctx = Context::new();
        let root = ctx.root.clone();
        let applies = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let fiber = fiber_in_creation(&ctx, applies.clone(), true);

        // the convergence winner cannot absorb the failure either
        assert!(fiber.slot.try_claim());
        let target = fiber.compute_target(&root);
        fiber.settle_once(&target, false).await;
        fiber.slot.abandon();

        // the initial pass applies once, fails, and reports InitialApply —
        // no won-back second attempt can swallow it
        let outcome = fiber.slot.initial_spawn_pass(&fiber, &root).await;
        assert!(
            matches!(outcome, super::InitialOutcome::Failed(_)),
            "the first apply's failure is the creation's own ending"
        );
        assert_eq!(
            applies.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "no second apply ran"
        );
        assert!(!fiber.is_alive(), "the attempted fiber was torn down");
    }

    #[tokio::test]
    async fn detached_convergence_invariant_panic_is_reported_and_remains_panicked() {
        let ctx = Context::new();
        let buffer = Arc::new(BufferExporter::new(8, Level::Debug).unwrap());
        let _registration = ctx.add_exporter(buffer.clone()).unwrap();
        let fiber = fiber_requiring(&ctx, &[]);

        // Manufacture the framework-only impossible shape: an ACTIVE
        // convergence slot without the SemanticTarget every real claimant
        // records before spawning the pass. The task must fail loudly; the
        // reporting boundary may add diagnostics but must not repair the slot.
        assert!(fiber.slot.try_claim());
        let join = super::InertiaSlot::spawn_convergence_pass(
            &tokio::runtime::Handle::current(),
            &fiber,
            &ctx.root,
        );

        let error = join
            .await
            .expect_err("the invariant panic still ends the task");
        assert!(error.is_panic());
        assert!(
            !fiber.slot.is_idle(),
            "framework panic reporting must not synthesize a recovery transition"
        );

        let records = buffer.snapshot();
        assert_eq!(
            records.len(),
            1,
            "one structured report per framework panic"
        );
        assert_eq!(records[0].level(), Level::Error);
        assert_eq!(records[0].channel(), "test");
        assert!(
            records[0]
                .text()
                .contains("framework task fiber convergence panicked")
        );
        assert!(records[0].text().contains("fiber=\"test\""));
        assert!(
            records[0]
                .text()
                .contains("an active convergence pass has a recorded SemanticTarget")
        );

        // Test-only cleanup after proving that production reporting did not
        // recover the corrupt protocol state.
        fiber.slot.abandon();
    }

    #[tokio::test]
    async fn a_convergence_applies_a_never_applied_fiber_after_handoff() {
        let ctx = Context::new();
        let root = ctx.root.clone();
        let applies = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let fiber = fiber_in_creation(&ctx, applies.clone(), false);
        // the handoff closed the creation window (stable-Pending delivery)
        fiber
            .creation_pending
            .store(false, std::sync::atomic::Ordering::SeqCst);

        assert!(fiber.slot.try_claim());
        let target = fiber.compute_target(&root);
        fiber.settle_once(&target, false).await;
        fiber.slot.abandon();
        assert_eq!(
            applies.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "post-handoff convergence runs the first apply normally"
        );
        assert_eq!(fiber.state(), FiberState::Active);
    }

    #[test]
    fn semantic_target_uses_fiber_edges_and_exact_publication_assignments() {
        let ctx = Context::new();
        let fiber = fiber_requiring(&ctx, &["counter"]);
        let missing = fiber.compute_target(&ctx.root);
        assert!(missing.has_missing());

        struct Counter;
        impl crate::Service for Counter {
            const NAME: &'static str = "counter";
        }
        let publication = ctx.provide(Arc::new(Counter)).unwrap();
        let present = fiber.compute_target(&ctx.root);
        assert!(!present.has_missing());
        assert_ne!(missing, present);

        publication.set(Arc::new(Counter)).unwrap();
        assert_eq!(
            present,
            fiber.compute_target(&ctx.root),
            "same publication payload mutation is target-neutral"
        );
    }

    #[test]
    fn target_neutral_recheck_during_release_is_acknowledged_without_another_pass() {
        let ctx = Context::new();
        let fiber = fiber_requiring(&ctx, &[]);
        assert!(fiber.slot.try_claim());
        let target = fiber.compute_target(&ctx.root);
        let revision = fiber.slot.recheck_revision();

        fiber.slot.commit_recheck();

        assert!(
            !fiber
                .slot
                .exit_recheck(&fiber, &ctx.root, &target, revision),
            "mechanism-only recheck revisions are not semantic target drift"
        );
        assert!(fiber.slot.is_idle());
        assert!(!fiber.slot.has_committed_recheck());
    }

    #[tokio::test]
    async fn target_neutral_idle_recheck_does_not_reapply_the_settled_target() {
        let ctx = Context::new();
        let root = ctx.root.clone();
        let applies = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let fiber = fiber_in_creation(&ctx, applies.clone(), false);
        let outcome = fiber.slot.initial_spawn_pass(&fiber, &root).await;
        assert!(matches!(outcome, super::InitialOutcome::Active));
        fiber
            .creation_pending
            .store(false, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(applies.load(std::sync::atomic::Ordering::SeqCst), 1);

        fiber.slot.commit_recheck();
        fiber.slot.kick(&fiber, &root);
        fiber.slot.wait_idle().await;

        assert_eq!(
            applies.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "same SemanticTarget rechecks must not trigger another apply"
        );
        assert!(!fiber.slot.has_committed_recheck());
    }

    #[test]
    fn index_state_is_not_needed_to_reconstruct_pending_diagnostics_or_target() {
        let ctx = Context::new();
        let fiber = fiber_requiring(&ctx, &["counter"]);
        ctx.root.deps.disable_for_test();
        assert_eq!(fiber.missing_from(&ctx.root), vec!["counter".to_owned()]);
        assert!(fiber.compute_target(&ctx.root).has_missing());
    }

    #[test]
    fn off_runtime_recheck_claims_releasing_slot_for_the_existing_holder() {
        let ctx = Context::new();
        let fiber = fiber_requiring(&ctx, &["counter"]);
        fiber.slot.inertia.store(
            super::INERTIA_RELEASING,
            std::sync::atomic::Ordering::SeqCst,
        );
        fiber.slot.commit_recheck();

        // Plain #[test]: there is no Tokio handle. The mutation still closes
        // the release race by returning RELEASING -> ACTIVE; it does not need
        // to spawn a second driver.
        fiber.slot.kick(&fiber, &ctx.root);
        assert_eq!(
            fiber.slot.inertia.load(std::sync::atomic::Ordering::SeqCst),
            super::INERTIA_ACTIVE
        );
        assert!(fiber.slot.has_committed_recheck());
        fiber.slot.abandon();
    }

    #[tokio::test]
    async fn abandon_wakes_waiters() {
        let ctx = Context::new();
        let fiber = fiber_requiring(&ctx, &[]);
        fiber.slot.claim().await;
        let waiter = tokio::spawn({
            let fiber = fiber.clone();
            async move { fiber.slot.wait_idle().await }
        });
        fiber.slot.abandon();
        bounded(2000, waiter)
            .await
            .expect("every IDLE store owes a notify_waiters")
            .unwrap();
    }

    #[tokio::test]
    async fn initial_spawn_pass_stays_pending_and_releases_when_services_missing() {
        let ctx = Context::new();
        let root = ctx.root.clone();
        // requires "counter", which nothing provides: target has Missing
        let fiber = fiber_requiring(&ctx, &["counter"]);
        let outcome = fiber.slot.initial_spawn_pass(&fiber, &root).await;
        assert!(
            matches!(outcome, super::InitialOutcome::Pending),
            "missing requirements end the pass as stable Pending"
        );
        assert_eq!(fiber.state(), FiberState::Pending);
        // the Pending arm's guarded write ran — a bare abandon path would
        // have left the cell at the untouched initial semantic target
        assert!(fiber.slot.current_target().has_missing());
        // the Pending path releases the arbiter too — the slot reads
        // idle, or a spawn racing a provide would hang on the claim
        assert!(fiber.slot.is_idle());
    }

    #[tokio::test]
    async fn initial_spawn_pass_on_an_invalidated_fiber_reports_interrupted() {
        let ctx = Context::new();
        let root = ctx.root.clone();
        // A Fiber the framework invalidated before the pass claimed:
        // attach it, then complete its ordinary terminal disposal before the
        // initial pass ever runs. Typed detach/removal is covered separately
        // by the handoff race and Issue 30 Registry-removal evidence.
        let fiber = fiber_requiring(&ctx, &[]);
        root.registry
            .attach_fiber(crate::registry::PluginKey::Anonymous(1), fiber.clone());
        fiber.dispose().await;

        let outcome = fiber.slot.initial_spawn_pass(&fiber, &root).await;
        assert!(
            matches!(outcome, super::InitialOutcome::Interrupted),
            "invalidation before the pass reports Interrupted"
        );
        assert!(fiber.slot.is_idle(), "the arbiter is released");
        assert_eq!(
            root.registry.snapshot_fibers().len(),
            0,
            "no resident attempted Fiber remains observable"
        );
    }

    #[test]
    fn changed_committed_effective_input_changes_the_semantic_target() {
        let ctx = Context::new();
        let root = ctx.root.clone();
        let applies = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let fiber = fiber_in_creation(&ctx, applies, false);
        let before = fiber.compute_target(&root);
        assert_eq!(
            before,
            fiber.compute_target(&root),
            "plain restart with the same committed input is target-neutral"
        );

        fiber
            .spawn_state
            .commit_change(crate::PreparedChange::from_input::<CountApplies>(()))
            .unwrap();

        let after = fiber.compute_target(&root);
        assert_ne!(
            before, after,
            "a committed apply-input change is target drift"
        );
    }

    #[test]
    fn normalized_declaration_order_produces_the_same_exact_edges() {
        let a = crate::InjectSpec::none().require("beta").require("alpha");
        let b = crate::InjectSpec::none().require("alpha").require("beta");
        let edges = |spec: &crate::InjectSpec| {
            spec.entries()
                .map(|entry| {
                    crate::fiber::DependencyEdge::new(entry.name.clone(), RealmKey::DEFAULT)
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(edges(&a), edges(&b));
    }
}
