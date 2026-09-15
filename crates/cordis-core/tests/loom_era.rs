//! Reduced Loom models for fresh-era replacement arbitration.
//!
//! These are Phase 3 reference models from #99. They do not execute Tokio or
//! production `FiberHandle::era_swap`. The production lifecycle slot is reduced
//! to a mutex because Phase 1/2 already cover unique slot ownership and waiter
//! signaling; this layer models the Era-specific terminal claim and successor
//! responsibility that run while/after that slot is owned.

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
#[should_panic]
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
#[should_panic]
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
#[should_panic]
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
#[should_panic]
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

const HANDOFF_WAITING: usize = 0;
const HANDOFF_CANCELLED: usize = 1;
const HANDOFF_DELIVERED: usize = 2;

/// Reduced ownership model for the `EraHandoffGuard`/oneshot seam. A prepared
/// successor starts framework-owned. The caller and framework race only over who
/// crosses the handoff boundary first; after either linearization, ownership is
/// no longer shared.
struct HandoffOwnership {
    state: AtomicUsize,
    cleanup_claims: AtomicUsize,
    resident: AtomicBool,
}

impl HandoffOwnership {
    fn new() -> Self {
        Self {
            state: AtomicUsize::new(HANDOFF_WAITING),
            cleanup_claims: AtomicUsize::new(0),
            resident: AtomicBool::new(true),
        }
    }

    /// Framework completes the offer. If the receiver already disappeared, the
    /// guard owns exactly one terminal cleanup; otherwise the successor is
    /// handed off and the guard is disarmed synchronously.
    fn offer(&self) {
        match self.state.compare_exchange(
            HANDOFF_WAITING,
            HANDOFF_DELIVERED,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            Ok(_) => {}
            Err(HANDOFF_CANCELLED) => {
                self.cleanup_claims.fetch_add(1, Ordering::SeqCst);
                self.resident.store(false, Ordering::SeqCst);
            }
            Err(other) => panic!("invalid handoff state {other}"),
        }
    }

    /// Caller cancellation before delivery closes only the receive side. If
    /// delivery already committed, the successor has escaped framework cleanup
    /// ownership and cannot be reclaimed by cancellation.
    fn cancel(&self) {
        match self.state.compare_exchange(
            HANDOFF_WAITING,
            HANDOFF_CANCELLED,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            Ok(_) | Err(HANDOFF_DELIVERED) => {}
            Err(other) => panic!("invalid handoff state {other}"),
        }
    }

    /// Negative variant: receiver cancellation wins, but the framework drops
    /// its cleanup responsibility instead of running the handoff guard.
    fn offer_without_cancelled_cleanup(&self) {
        let _ = self.state.compare_exchange(
            HANDOFF_WAITING,
            HANDOFF_DELIVERED,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }

    /// Negative variant: cancellation after committed delivery incorrectly
    /// reclaims a successor that already belongs to the caller.
    fn cancel_and_reclaim_delivered(&self) {
        if self.state.load(Ordering::SeqCst) == HANDOFF_DELIVERED {
            self.cleanup_claims.fetch_add(1, Ordering::SeqCst);
            self.resident.store(false, Ordering::SeqCst);
        } else {
            self.cancel();
        }
    }
}

fn assert_resolved_handoff(handoff: &HandoffOwnership) {
    match handoff.state.load(Ordering::SeqCst) {
        HANDOFF_DELIVERED => {
            assert_eq!(
                handoff.cleanup_claims.load(Ordering::SeqCst),
                0,
                "delivered successor cannot retain framework cleanup ownership"
            );
            assert!(
                handoff.resident.load(Ordering::SeqCst),
                "delivered successor was reclaimed after handoff"
            );
        }
        HANDOFF_CANCELLED => {
            assert_eq!(
                handoff.cleanup_claims.load(Ordering::SeqCst),
                1,
                "cancelled handoff must assign exactly one cleanup owner"
            );
            assert!(
                !handoff.resident.load(Ordering::SeqCst),
                "cancelled handoff left an undelivered successor resident"
            );
        }
        other => panic!("handoff stayed unresolved in state {other}"),
    }
}

/// ER-07: caller cancellation and framework handoff race to one linearized
/// ownership result. Cancellation-first leaves exactly one cleanup obligation;
/// handoff-first leaves the successor resident and no framework cleanup claim.
#[test]
fn caller_cancellation_and_handoff_assign_one_successor_owner() {
    era_model(|| {
        let handoff = Arc::new(HandoffOwnership::new());
        let offering = {
            let handoff = handoff.clone();
            thread::spawn(move || handoff.offer())
        };
        let cancelling = {
            let handoff = handoff.clone();
            thread::spawn(move || handoff.cancel())
        };

        offering.join().unwrap();
        cancelling.join().unwrap();
        assert_resolved_handoff(&handoff);
    });
}

/// ER-07 negative control: cancellation-first without guard cleanup leaves an
/// undelivered successor resident and loses the committed cleanup obligation.
/// Loom must find the schedule where cancellation wins before the bad offer.
#[test]
#[should_panic]
fn cancelled_handoff_without_framework_cleanup_is_detected() {
    era_model(|| {
        let handoff = Arc::new(HandoffOwnership::new());
        let offering = {
            let handoff = handoff.clone();
            thread::spawn(move || handoff.offer_without_cancelled_cleanup())
        };
        let cancelling = {
            let handoff = handoff.clone();
            thread::spawn(move || handoff.cancel())
        };

        offering.join().unwrap();
        cancelling.join().unwrap();
        assert_resolved_handoff(&handoff);
    });
}

/// ER-07 negative control: once delivery commits, caller cancellation cannot
/// regain cleanup authority over the published successor. Loom must find the
/// schedule where delivery wins before the bad cancellation path.
#[test]
#[should_panic]
fn cancellation_reclaiming_an_already_handed_off_successor_is_detected() {
    era_model(|| {
        let handoff = Arc::new(HandoffOwnership::new());
        let offering = {
            let handoff = handoff.clone();
            thread::spawn(move || handoff.offer())
        };
        let cancelling = {
            let handoff = handoff.clone();
            thread::spawn(move || handoff.cancel_and_reclaim_delivered())
        };

        offering.join().unwrap();
        cancelling.join().unwrap();
        assert_resolved_handoff(&handoff);
    });
}
