//! Reduced Loom models for fresh-era replacement arbitration.
//!
//! These are Phase 3 reference models from #99. They do not execute Tokio or
//! production `FiberHandle::era_swap`. The production lifecycle slot is reduced
//! to a mutex because Phase 1/2 already cover unique slot ownership and waiter
//! signaling; this layer models the Era-specific terminal claim and successor
//! responsibility that run while/after that slot is owned.

use loom::cell::UnsafeCell;
use loom::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use loom::sync::{Arc, Mutex};
use loom::thread;

fn era_model<F>(f: F)
where
    F: Fn() + Sync + Send + 'static,
{
    let mut builder = loom::model::Builder::new();
    builder.max_threads = 3; // main + two competing/observing actors
    builder.max_branches = 64;
    builder.max_permutations = None;
    builder.max_duration = None;
    builder.check(f);
}

/// Reduced production mapping for ADR 0030's source arbitration.
///
/// `FiberHandle::era_swap` first waits for `InertiaSlot::claim()`. Under that
/// exclusive lifecycle slot it wins the source only through
/// `disposing.swap(true)`. A loser releases the slot and returns `Closed`
/// before `run_committed_replacement` can attempt a successor.
struct EraArbitration {
    lifecycle: Mutex<()>,
    disposing: AtomicBool,
    swap_claims: AtomicUsize,
    dispose_claims: AtomicUsize,
    successor_attempts: AtomicUsize,
}

impl EraArbitration {
    fn new() -> Self {
        Self {
            lifecycle: Mutex::new(()),
            disposing: AtomicBool::new(false),
            swap_claims: AtomicUsize::new(0),
            dispose_claims: AtomicUsize::new(0),
            successor_attempts: AtomicUsize::new(0),
        }
    }

    /// Serialize one terminal owner and commit the shared Open-to-Closing
    /// source claim. The counter identifies which operation won that authority.
    fn claim_source(&self, claims: &AtomicUsize) -> bool {
        let _lifecycle = self.lifecycle.lock().unwrap();
        if self.disposing.swap(true, Ordering::SeqCst) {
            return false;
        }
        claims.fetch_add(1, Ordering::SeqCst);
        true
    }

    /// Positive Era path: claim the serialized lifecycle owner, commit the
    /// source terminal claim, then and only then attempt the one successor.
    fn swap(&self) -> bool {
        if !self.claim_source(&self.swap_claims) {
            return false;
        }

        // Production releases the source lifecycle slot as part of completing
        // the old terminal barrier before successor creation begins. Do not
        // accidentally serialize candidate work behind this reduced slot.
        self.successor_attempts.fetch_add(1, Ordering::SeqCst);
        true
    }

    /// Ordinary terminal disposal shares the same live-source arbitration but
    /// never creates an Era successor.
    fn dispose(&self) -> bool {
        self.claim_source(&self.dispose_claims)
    }

    /// Negative variant for the current no-retry contract: one successful
    /// source claim attempts two successor candidates.
    fn swap_with_successor_retry(&self) -> bool {
        if !self.claim_source(&self.swap_claims) {
            return false;
        }

        self.successor_attempts.fetch_add(2, Ordering::SeqCst);
        true
    }

    /// Negative variant: speculative successor allocation happens from a stale
    /// preclaim liveness read, before lifecycle serialization and terminal
    /// ownership. Two racers can therefore both allocate even though only one
    /// later wins the source.
    fn swap_with_preclaim_successor_attempt(&self) -> bool {
        if !self.disposing.load(Ordering::SeqCst) {
            thread::yield_now();
            self.successor_attempts.fetch_add(1, Ordering::SeqCst);
        }

        self.claim_source(&self.swap_claims)
    }
}

/// ER-01 / ER-02 / ER-03: two racing swaps may choose either winner, but the
/// live source grants exactly one terminal claim, the loser allocates nothing,
/// and the successful no-retry transaction attempts exactly one successor.
#[test]
fn racing_swaps_grant_one_source_claim_and_one_successor_attempt() {
    era_model(|| {
        let era = Arc::new(EraArbitration::new());
        let left = {
            let era = era.clone();
            thread::spawn(move || era.swap())
        };
        let right = {
            let era = era.clone();
            thread::spawn(move || era.swap())
        };

        let left_won = left.join().unwrap();
        let right_won = right.join().unwrap();
        assert_ne!(left_won, right_won, "exactly one swap owns the live source");
        assert_eq!(era.swap_claims.load(Ordering::SeqCst), 1);
        assert_eq!(
            era.successor_attempts.load(Ordering::SeqCst),
            1,
            "the losing swap must return Closed before successor allocation"
        );
    });
}

/// ER-02 negative control. Moving successor allocation ahead of the serialized
/// source claim lets both racers observe the source as open and allocate. Loom
/// must find that schedule; otherwise the model cannot discriminate the
/// allocation-before-claim defect ADR 0030 forbids.
#[test]
#[should_panic(expected = "a loser allocated a successor before owning the source")]
fn preclaim_successor_allocation_is_detected() {
    era_model(|| {
        let era = Arc::new(EraArbitration::new());
        let left = {
            let era = era.clone();
            thread::spawn(move || era.swap_with_preclaim_successor_attempt())
        };
        let right = {
            let era = era.clone();
            thread::spawn(move || era.swap_with_preclaim_successor_attempt())
        };

        left.join().unwrap();
        right.join().unwrap();
        assert_eq!(
            era.successor_attempts.load(Ordering::SeqCst),
            era.swap_claims.load(Ordering::SeqCst),
            "a loser allocated a successor before owning the source"
        );
    });
}

/// ER-03 negative control. The current Era contract is no-retry: once a source
/// claim commits, exactly one successor candidate may be attempted. A model that
/// retries candidate creation must be rejected even though terminal ownership
/// itself remains unique.
#[test]
#[should_panic(expected = "one source claim retried successor allocation")]
fn successor_retry_after_one_source_claim_is_detected() {
    era_model(|| {
        let era = Arc::new(EraArbitration::new());
        let owner = {
            let era = era.clone();
            thread::spawn(move || era.swap_with_successor_retry())
        };

        assert!(owner.join().unwrap());
        assert_eq!(era.swap_claims.load(Ordering::SeqCst), 1);
        assert_eq!(
            era.successor_attempts.load(Ordering::SeqCst),
            1,
            "one source claim retried successor allocation"
        );
    });
}

/// ER-01 / ER-02 against ordinary disposal. Whichever operation first owns the
/// serialized lifecycle slot may commit Open-to-Closing. A swap may attempt a
/// successor iff that swap, rather than ordinary disposal, owns that one claim.
#[test]
fn disposal_race_allows_successor_only_for_a_winning_swap() {
    era_model(|| {
        let era = Arc::new(EraArbitration::new());
        let swapping = {
            let era = era.clone();
            thread::spawn(move || era.swap())
        };
        let disposing = {
            let era = era.clone();
            thread::spawn(move || era.dispose())
        };

        let swap_won = swapping.join().unwrap();
        let dispose_won = disposing.join().unwrap();
        assert_ne!(swap_won, dispose_won, "one terminal owner must win");
        assert_eq!(
            era.swap_claims.load(Ordering::SeqCst) + era.dispose_claims.load(Ordering::SeqCst),
            1,
            "source terminal ownership is unique"
        );
        assert_eq!(
            era.successor_attempts.load(Ordering::SeqCst),
            usize::from(swap_won),
            "a swap losing to ordinary disposal must allocate no successor"
        );
    });
}

/// ER-04 production mapping: `run_committed_replacement` awaits
/// `source.complete_claimed_dispose()` before converting the captured recipe or
/// calling `spawn_prepared_era_successor`. Seeing a successor attempt therefore
/// implies the full old-Fiber terminal barrier already completed.
#[test]
fn successor_attempt_observes_completed_source_disposal() {
    era_model(|| {
        let source_complete = Arc::new(AtomicBool::new(false));
        let successor_attempted = Arc::new(AtomicBool::new(false));

        let owner = {
            let source_complete = source_complete.clone();
            let successor_attempted = successor_attempted.clone();
            thread::spawn(move || {
                source_complete.store(true, Ordering::Release);
                successor_attempted.store(true, Ordering::Release);
            })
        };
        let observer = {
            let source_complete = source_complete.clone();
            let successor_attempted = successor_attempted.clone();
            thread::spawn(move || {
                if successor_attempted.load(Ordering::Acquire) {
                    assert!(
                        source_complete.load(Ordering::Acquire),
                        "successor birth became observable before old-Fiber death completed"
                    );
                }
            })
        };

        owner.join().unwrap();
        observer.join().unwrap();
        assert!(successor_attempted.load(Ordering::SeqCst));
        assert!(source_complete.load(Ordering::SeqCst));
    });
}

/// ER-04 negative control. This deliberately restores birth-before-death:
/// successor creation is published, the owner yields, and only later completes
/// source disposal. Loom must expose an observer in that forbidden middle state.
#[test]
#[should_panic(expected = "birth-before-death window reproduced")]
fn successor_attempt_before_source_disposal_is_detected() {
    era_model(|| {
        let source_complete = Arc::new(AtomicBool::new(false));
        let successor_attempted = Arc::new(AtomicBool::new(false));

        let owner = {
            let source_complete = source_complete.clone();
            let successor_attempted = successor_attempted.clone();
            thread::spawn(move || {
                successor_attempted.store(true, Ordering::Release);
                thread::yield_now();
                source_complete.store(true, Ordering::Release);
            })
        };
        let observer = {
            let source_complete = source_complete.clone();
            let successor_attempted = successor_attempted.clone();
            thread::spawn(move || {
                if successor_attempted.load(Ordering::Acquire) {
                    assert!(
                        source_complete.load(Ordering::Acquire),
                        "birth-before-death window reproduced"
                    );
                }
            })
        };

        owner.join().unwrap();
        observer.join().unwrap();
    });
}

/// ER-05 positive path: once the swap owns the terminal source claim, the fact
/// that `disposing` is now true is the *result of that claim*, not a reason to
/// invalidate the already-captured Era recipe. Production captures the recipe
/// before admission, then lets the detached committed owner use it after source
/// closure and complete old-Fiber death.
#[test]
fn committed_swap_continues_with_its_captured_recipe_after_closure() {
    era_model(|| {
        let era = Arc::new(EraArbitration::new());
        let recipe_captured = Arc::new(AtomicBool::new(false));

        let owner = {
            let era = era.clone();
            let recipe_captured = recipe_captured.clone();
            thread::spawn(move || {
                recipe_captured.store(true, Ordering::SeqCst);
                assert!(era.claim_source(&era.swap_claims));
                assert!(era.disposing.load(Ordering::SeqCst));
                assert!(recipe_captured.load(Ordering::SeqCst));
                era.successor_attempts.fetch_add(1, Ordering::SeqCst);
            })
        };

        owner.join().unwrap();
        assert_eq!(era.swap_claims.load(Ordering::SeqCst), 1);
        assert_eq!(era.successor_attempts.load(Ordering::SeqCst), 1);
    });
}

/// ER-05: a source that was already closing before Era admission cannot mint a
/// new replacement owner or candidate.
#[test]
fn preclosed_source_cannot_authorize_a_new_swap() {
    era_model(|| {
        let era = Arc::new(EraArbitration::new());
        era.disposing.store(true, Ordering::SeqCst);

        assert!(!era.swap());
        assert_eq!(era.swap_claims.load(Ordering::SeqCst), 0);
        assert_eq!(era.successor_attempts.load(Ordering::SeqCst), 0);
    });
}

/// ER-05 negative control. Re-checking `disposing` as though it were external
/// closure *after* this swap has successfully set it would cause the committed
/// owner to reject its own captured recipe and strand an already-ended source.
#[test]
#[should_panic(
    expected = "committed Era owner abandoned the recipe because its own claim closed the source"
)]
fn postclaim_closed_recheck_that_abandons_the_captured_recipe_is_detected() {
    era_model(|| {
        let era = Arc::new(EraArbitration::new());
        let recipe_captured = true;
        assert!(era.claim_source(&era.swap_claims));

        if recipe_captured && !era.disposing.load(Ordering::SeqCst) {
            era.successor_attempts.fetch_add(1, Ordering::SeqCst);
        }

        assert_eq!(
            era.successor_attempts.load(Ordering::SeqCst),
            1,
            "committed Era owner abandoned the recipe because its own claim closed the source"
        );
    });
}

// ER-07 handoff: offer / accept / cancel.
//
// Production (`FiberHandle::era_swap` in `src/fiber/era.rs`) splits a
// successful handoff into two stages. The committed replacement task builds
// `EraHandoff { fiber_handle, guard }` around an armed `EraHandoffGuard` and
// offers it with `let _ = send.send(Ok(offer));`. A successful send transfers no
// ownership: the armed guard travels inside the offered message. Only the
// caller's `handoff.accept()` after `receive.await` disarms it. Dropping the
// offered message unaccepted, whether the sender gets it back from a closed
// receiver or a dropped receiver releases it, runs `Drop for EraHandoffGuard`,
// which detaches `cleanup_undelivered_successor`.
//
// The model therefore keeps the real message ownership: the guard is a value
// with a `Drop`, and it moves through a reduced oneshot. The oneshot keeps Tokio
// 1.53's `VALUE_SENT`/`CLOSED` state-word protocol and memory orderings, so the
// send/receive edge is the one production relies on. No mutex or `SeqCst`
// oracle access is added on that path.

/// Reduced `tokio::sync::oneshot::State` bit: `Sender::send` stored the offer.
const VALUE_SENT: usize = 0b01;
/// Reduced `tokio::sync::oneshot::State` bit: the `Receiver` was dropped.
const CLOSED: usize = 0b10;

/// Oracle-only facts about the one prepared successor. The protocol never reads
/// them. They use `Relaxed` accesses, which create no happens-before edge, and
/// the main thread reads them only after both joins.
struct SuccessorFacts {
    cleanups: AtomicUsize,
}

impl SuccessorFacts {
    fn new() -> Self {
        Self {
            cleanups: AtomicUsize::new(0),
        }
    }

    /// One detached `cleanup_undelivered_successor`: the successor is disposed
    /// and is no longer resident.
    fn clean(&self) {
        self.cleanups.fetch_add(1, Ordering::Relaxed);
    }
}

/// `EraHandoffGuard`. Production's `successor: Option<Arc<Fiber>>` is reduced
/// to the successor's oracle facts. `Some` means armed.
struct HandoffGuard {
    successor: Option<Arc<SuccessorFacts>>,
}

impl HandoffGuard {
    /// `EraHandoffGuard::new`: the guard is armed when it is built.
    fn armed(successor: Arc<SuccessorFacts>) -> Self {
        Self {
            successor: Some(successor),
        }
    }

    /// `EraHandoffGuard::disarm`: `self.successor.take()`.
    fn disarm(mut self) {
        self.successor.take();
    }
}

impl Drop for HandoffGuard {
    /// `impl Drop for EraHandoffGuard`: an armed guard detaches
    /// `cleanup_undelivered_successor` for its successor; a disarmed guard does
    /// nothing. The `Option::take` makes the cleanup claim single-use.
    fn drop(&mut self) {
        if let Some(successor) = self.successor.take() {
            successor.clean();
        }
    }
}

/// `EraHandoff { fiber_handle, guard }`, the offered message. The handle itself
/// carries no cleanup authority, so the model keeps only the guard.
struct EraOffer {
    guard: HandoffGuard,
}

impl EraOffer {
    /// `EraHandoff::accept`: `self.guard.disarm()` and return the handle. After
    /// this the caller owns the successor and no framework cleanup remains.
    fn accept(self) {
        self.guard.disarm();
    }
}

/// Reduced `tokio::sync::oneshot` carrying one `EraOffer`. Waker bits are
/// omitted: the model's caller polls once instead of parking.
struct OfferChannel {
    state: AtomicUsize,
    value: UnsafeCell<Option<EraOffer>>,
}

// SAFETY: as in Tokio's oneshot, `value` is written only by the sender before
// it publishes `VALUE_SENT`, taken back by the sender only after its CAS saw
// `CLOSED` (so `VALUE_SENT` is never set), and otherwise taken only by the
// receiver after it acquires `VALUE_SENT`. Loom's `UnsafeCell` checks these
// accesses for causality in every explored schedule.
unsafe impl Sync for OfferChannel {}

impl OfferChannel {
    fn new() -> Self {
        Self {
            state: AtomicUsize::new(0),
            value: UnsafeCell::new(None),
        }
    }

    /// `Sender::send`: store the offer, then `Inner::complete`'s
    /// `State::set_complete` CAS (`AcqRel`/`Acquire`). If the receiver already
    /// closed, the sender takes the offer back and returns it as `Err`.
    fn send(&self, offer: EraOffer) -> Result<(), EraOffer> {
        // SAFETY: `VALUE_SENT` is not set yet, so the receiver does not touch
        // the cell.
        self.value.with_mut(|slot| unsafe { *slot = Some(offer) });
        let mut state = self.state.load(Ordering::Relaxed);
        loop {
            if state & CLOSED != 0 {
                // SAFETY: the CAS never set `VALUE_SENT`, so the receiver will
                // never access the cell.
                let refused = self.value.with_mut(|slot| unsafe { (*slot).take() });
                return Err(refused.expect("the sender stored its offer"));
            }
            match self.state.compare_exchange(
                state,
                state | VALUE_SENT,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(()),
                Err(actual) => state = actual,
            }
        }
    }

    /// One poll of `receive.await` (`Inner::poll_recv`): an `Acquire` load and,
    /// if the offer was sent, consuming it. Production's `.await` consumes the
    /// receiver on `Ready`, so its later drop releases nothing.
    fn try_recv(&self) -> Option<EraOffer> {
        if self.state.load(Ordering::Acquire) & VALUE_SENT == 0 {
            return None;
        }
        // SAFETY: `VALUE_SENT` was acquired, so the sender no longer accesses
        // the cell.
        self.value.with_mut(|slot| unsafe { (*slot).take() })
    }

    /// `Drop for Receiver`: `Inner::close` (`fetch_or(CLOSED, Acquire)`), then
    /// drop any offer that was already sent. Returns whether an offered message
    /// was released this way.
    fn close(&self) -> bool {
        let previous = self.state.fetch_or(CLOSED, Ordering::Acquire);
        if previous & VALUE_SENT == 0 {
            return false;
        }
        // SAFETY: `VALUE_SENT` was acquired, so the sender no longer accesses
        // the cell.
        let offered = self.value.with_mut(|slot| unsafe { (*slot).take() });
        drop(offered);
        true
    }
}

/// Terminal ownership outcome, classified by the caller's own action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HandoffOutcome {
    /// The caller received the offer and called `accept`.
    Accepted,
    /// The receiver closed before `VALUE_SENT`; the sender gets the offer back.
    CancelledBeforeOffer,
    /// The receiver closed after `VALUE_SENT` and released the unaccepted offer.
    CancelledAfterOffer,
}

impl HandoffOutcome {
    const ALL: usize = 0b111;

    fn bit(self) -> usize {
        match self {
            Self::Accepted => 0b001,
            Self::CancelledBeforeOffer => 0b010,
            Self::CancelledAfterOffer => 0b100,
        }
    }
}

/// Framework side, production success arm of the committed replacement task:
/// `EraHandoffGuard::new(..)`, `EraHandoff { fiber_handle, guard }`, then
/// `let _ = send.send(Ok(offer));`. A refused offer is dropped with its guard
/// still armed. Returns whether the send succeeded, which is not acceptance.
fn offer(channel: &OfferChannel, successor: &Arc<SuccessorFacts>) -> bool {
    let offer = EraOffer {
        guard: HandoffGuard::armed(successor.clone()),
    };
    channel.send(offer).is_ok()
}

/// Caller side: one poll of `receive.await` that, on `Ready`, runs
/// `handoff.accept()` in the same poll, as production does. Otherwise the
/// caller cancels: dropping the `era_swap` future drops the receiver. The
/// close may land before the offer or after it.
fn accept_or_cancel(channel: &OfferChannel, _successor: &Arc<SuccessorFacts>) -> HandoffOutcome {
    if let Some(offer) = channel.try_recv() {
        offer.accept();
        return HandoffOutcome::Accepted;
    }
    if channel.close() {
        HandoffOutcome::CancelledAfterOffer
    } else {
        HandoffOutcome::CancelledBeforeOffer
    }
}

/// Negative variant: when the receiver already closed, the framework disarms
/// the refused offer instead of letting its guard clean the successor.
fn offer_disarming_a_refused_offer(
    channel: &OfferChannel,
    successor: &Arc<SuccessorFacts>,
) -> bool {
    let offer = EraOffer {
        guard: HandoffGuard::armed(successor.clone()),
    };
    match channel.send(offer) {
        Ok(()) => true,
        Err(refused) => {
            refused.guard.disarm();
            false
        }
    }
}

/// Negative variant: treat a successful send as delivery, which is the earlier
/// one-step model. The offer leaves with a disarmed guard and the framework
/// cleans only a refused send, so a receiver dropped after the offer but before
/// `accept` releases the offered message without cleanup.
fn offer_treating_send_as_delivery(
    channel: &OfferChannel,
    successor: &Arc<SuccessorFacts>,
) -> bool {
    let offer = EraOffer {
        guard: HandoffGuard { successor: None },
    };
    match channel.send(offer) {
        Ok(()) => true,
        Err(_refused) => {
            successor.clean();
            false
        }
    }
}

/// Negative variant: after `accept`, the caller's later cancellation still holds
/// cleanup authority and reclaims the successor it now owns.
fn accept_then_reclaim_on_cancel(
    channel: &OfferChannel,
    successor: &Arc<SuccessorFacts>,
) -> HandoffOutcome {
    let outcome = accept_or_cancel(channel, successor);
    if outcome == HandoffOutcome::Accepted {
        successor.clean();
    }
    outcome
}

/// Check one terminal state: exactly one owner, either the accepting caller
/// (resident, no framework cleanup) or exactly one framework cleanup.
fn assert_resolved_handoff(outcome: HandoffOutcome, offered: bool, successor: &SuccessorFacts) {
    assert_eq!(
        offered,
        outcome != HandoffOutcome::CancelledBeforeOffer,
        "the caller outcome disagrees with the send result"
    );
    let cleanups = successor.cleanups.load(Ordering::Relaxed);
    match outcome {
        HandoffOutcome::Accepted => assert_eq!(
            cleanups, 0,
            "accepted successor cannot retain framework cleanup ownership"
        ),
        HandoffOutcome::CancelledBeforeOffer => assert_eq!(
            cleanups, 1,
            "cancelled handoff must assign exactly one cleanup owner"
        ),
        HandoffOutcome::CancelledAfterOffer => assert_eq!(
            cleanups, 1,
            "offered but unaccepted successor must be cleaned exactly once by its armed guard"
        ),
    }
    assert_eq!(
        usize::from(outcome == HandoffOutcome::Accepted) + cleanups,
        1,
        "terminal handoff must have exactly one successor owner"
    );
}

/// Shared ER-07 handoff range: main plus the framework offer and the caller,
/// under `era_model`'s bounds. Returns the set of terminal outcomes reached
/// across all explored schedules, as `HandoffOutcome::bit` flags.
fn handoff_model(
    framework: fn(&OfferChannel, &Arc<SuccessorFacts>) -> bool,
    caller: fn(&OfferChannel, &Arc<SuccessorFacts>) -> HandoffOutcome,
) -> usize {
    // Plain `std` atomic outside the model: it records outcomes across
    // executions and is never visible to the modeled threads.
    let reached = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let recorded = reached.clone();
    era_model(move || {
        let successor = Arc::new(SuccessorFacts::new());
        let channel = Arc::new(OfferChannel::new());
        let offering = {
            let channel = channel.clone();
            let successor = successor.clone();
            thread::spawn(move || framework(&channel, &successor))
        };
        let receiving = {
            let channel = channel.clone();
            let successor = successor.clone();
            thread::spawn(move || caller(&channel, &successor))
        };

        let offered = offering.join().unwrap();
        let outcome = receiving.join().unwrap();
        assert_resolved_handoff(outcome, offered, &successor);
        recorded.fetch_or(outcome.bit(), std::sync::atomic::Ordering::Relaxed);
    });
    reached.load(std::sync::atomic::Ordering::Relaxed)
}

/// ER-07: framework offer, caller accept and caller cancellation race. Every
/// terminal state has exactly one successor owner. Accept leaves the successor
/// resident with no framework cleanup. Cancellation before the offer, and
/// cancellation after the offer but before `accept`, each leave exactly one
/// guard cleanup. The model must reach all three outcomes, so the
/// offered-but-not-accepted state is explored, not assumed.
///
/// Real Tokio (Layer C) counterpart for the offered-but-not-accepted schedule:
/// `fiber::era::tests::unconsumed_success_offer_cleans_the_offered_successor`.
#[test]
fn caller_cancellation_and_handoff_assign_one_successor_owner() {
    let reached = handoff_model(offer, accept_or_cancel);
    assert_eq!(
        reached,
        HandoffOutcome::ALL,
        "handoff model did not reach every terminal outcome"
    );
}

/// ER-07 negative control: cancellation before the offer, with the framework
/// disarming the refused offer instead of cleaning, leaves an undelivered
/// successor resident. Loom must find the schedule where the receiver closes
/// before the send.
#[test]
#[should_panic(expected = "cancelled handoff must assign exactly one cleanup owner")]
fn cancelled_handoff_without_framework_cleanup_is_detected() {
    handoff_model(offer_disarming_a_refused_offer, accept_or_cancel);
}

/// ER-07 negative control: once the caller accepts, a later caller
/// cancellation cannot regain cleanup authority over the accepted successor.
/// Loom must find the schedule where the offer lands before the caller's poll.
#[test]
#[should_panic(expected = "accepted successor cannot retain framework cleanup ownership")]
fn cancellation_reclaiming_an_already_handed_off_successor_is_detected() {
    handoff_model(offer, accept_then_reclaim_on_cancel);
}

/// ER-07 negative control for the offered-but-not-accepted state: a successful
/// send is treated as delivery, so the offered message carries no armed guard.
/// Loom must find the schedule where the caller's poll misses the offer, the
/// send succeeds, and the receiver is then dropped, releasing the offer without
/// cleanup.
#[test]
#[should_panic(
    expected = "offered but unaccepted successor must be cleaned exactly once by its armed guard"
)]
fn offered_but_unaccepted_handoff_without_guard_cleanup_is_detected() {
    handoff_model(offer_treating_send_as_delivery, accept_or_cancel);
}
