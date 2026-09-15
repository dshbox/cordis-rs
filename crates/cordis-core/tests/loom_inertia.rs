//! Reduced Loom models for the Fiber convergence protocol.
//!
//! These tests intentionally do not pretend to execute Tokio or the production
//! `InertiaSlot`. They are Phase 1 reference models from #99. Each model names
//! the production transition it represents, and each historical negative
//! control must fail if the model is capable of observing the bug it claims to
//! guard against.

use loom::sync::atomic::{AtomicUsize, Ordering};
use loom::sync::{Arc, Mutex};
use loom::thread;

const IDLE: usize = 0;
const ACTIVE: usize = 1;
const RELEASING: usize = 2;

fn lc06_model<F>(f: F)
where
    F: Fn() + Sync + Send + 'static,
{
    let mut builder = loom::model::Builder::new();
    builder.max_threads = 3; // main + mutation + ready
    builder.max_branches = 64;
    // No permutation or duration cap: exhaust the declared finite range.
    builder.max_permutations = None;
    builder.max_duration = None;
    builder.check(f);
}

/// ADR 0031 / #101 production mapping:
///
/// `ServiceStore` semantic commit:
///     commit dependent revision
///     -> publish the new visible target while still under the same store lock
///
/// An observer that starts from the already-published semantic state must never
/// see that state without its durable obligation. Acquire/Release here models
/// the happens-before edge the ServiceStore critical section provides; the real
/// implementation currently uses stronger SeqCst revision atomics plus the
/// store mutex.
#[test]
fn semantic_commit_publishes_revision_before_visibility() {
    loom::model(|| {
        let revision = Arc::new(AtomicUsize::new(0));
        let target_version = Arc::new(AtomicUsize::new(0));

        let mutation = {
            let revision = revision.clone();
            let target_version = target_version.clone();
            thread::spawn(move || {
                revision.store(1, Ordering::Release);
                target_version.store(1, Ordering::Release);
            })
        };

        let observer = {
            let revision = revision.clone();
            let target_version = target_version.clone();
            thread::spawn(move || {
                while target_version.load(Ordering::Acquire) == 0 {
                    thread::yield_now();
                }
                assert_eq!(
                    revision.load(Ordering::Acquire),
                    1,
                    "published semantic target is visible without its durable recheck"
                );
            })
        };

        mutation.join().unwrap();
        observer.join().unwrap();
    });
}

/// Negative control for #101. This deliberately restores the pre-fix ordering:
/// publish Service visibility first, then commit the dependent revision.
///
/// The test is supposed to panic: Loom must find a schedule in which the
/// observer sees target version 1 while revision 0 is still visible. If this
/// starts passing, the model no longer discriminates the historical defect.
#[test]
#[should_panic]
fn historical_visibility_before_revision_is_detected() {
    loom::model(|| {
        let revision = Arc::new(AtomicUsize::new(0));
        let target_version = Arc::new(AtomicUsize::new(0));

        let mutation = {
            let revision = revision.clone();
            let target_version = target_version.clone();
            thread::spawn(move || {
                target_version.store(1, Ordering::Release);
                thread::yield_now();
                revision.store(1, Ordering::Release);
            })
        };

        let observer = {
            let revision = revision.clone();
            let target_version = target_version.clone();
            thread::spawn(move || {
                while target_version.load(Ordering::Acquire) == 0 {
                    thread::yield_now();
                }
                assert_eq!(
                    revision.load(Ordering::Acquire),
                    1,
                    "historical publication-to-revision window reproduced"
                );
            })
        };

        mutation.join().unwrap();
        observer.join().unwrap();
    });
}

/// LC-03 production mapping: target reconstruction takes the same ServiceStore
/// lock as the semantic commit, while `exit_recheck` reads target state before
/// the durable revision and loops when that revision changed. Therefore a
/// revision can be acknowledged only by an inspection that happened after the
/// corresponding semantic state became visible.
#[test]
fn revision_acknowledgement_has_covering_target_inspection() {
    loom::model(|| {
        let revision = Arc::new(AtomicUsize::new(0));
        let target_version = Arc::new(Mutex::new(0usize));

        let mutation = {
            let revision = revision.clone();
            let target_version = target_version.clone();
            thread::spawn(move || {
                let mut target = target_version.lock().unwrap();
                // #101 / ADR 0031 semantic commit: revision first, visibility
                // second, while target readers are excluded by this lock.
                revision.store(1, Ordering::SeqCst);
                *target = 1;
            })
        };

        let holder = {
            let revision = revision.clone();
            let target_version = target_version.clone();
            thread::spawn(move || {
                let mut covered_revision = 0usize;
                loop {
                    let inspected_target = *target_version.lock().unwrap();
                    let observed_revision = revision.load(Ordering::SeqCst);
                    if observed_revision != covered_revision {
                        covered_revision = observed_revision;
                        continue;
                    }

                    assert!(
                        inspected_target >= covered_revision,
                        "revision {covered_revision} was acknowledged using target version {inspected_target}"
                    );
                    break;
                }
            })
        };

        mutation.join().unwrap();
        holder.join().unwrap();
    });
}

/// LC-03 negative control. This deliberately removes `exit_recheck`'s loop on a
/// newly observed revision: the holder keeps an older target inspection, later
/// observes the newer durable revision, and incorrectly treats that old read as
/// coverage for the new revision. Loom must find that interleaving.
#[test]
#[should_panic]
fn premature_revision_acknowledgement_without_reinspection_is_detected() {
    loom::model(|| {
        let revision = Arc::new(AtomicUsize::new(0));
        let target_version = Arc::new(Mutex::new(0usize));

        let mutation = {
            let revision = revision.clone();
            let target_version = target_version.clone();
            thread::spawn(move || {
                let mut target = target_version.lock().unwrap();
                revision.store(1, Ordering::SeqCst);
                *target = 1;
            })
        };

        let holder = {
            let revision = revision.clone();
            let target_version = target_version.clone();
            thread::spawn(move || {
                let inspected_target = *target_version.lock().unwrap();
                thread::yield_now();
                let acknowledged_revision = revision.load(Ordering::SeqCst);
                assert!(
                    inspected_target >= acknowledged_revision,
                    "premature acknowledgement reproduced: revision {acknowledged_revision}, target version {inspected_target}"
                );
            })
        };

        mutation.join().unwrap();
        holder.join().unwrap();
    });
}

/// Production mapping: `InertiaSlot::exit_recheck` moves ACTIVE -> RELEASING,
/// performs the final semantic/revision recheck, then alone may publish IDLE.
/// This structural model checks the central safety property without yet
/// modeling Tokio notification or target reconstruction.
#[test]
fn guarded_release_never_publishes_idle_during_recheck() {
    loom::model(|| {
        const NOT_STARTED: usize = 0;
        const RECHECKING: usize = 1;
        const COMPLETE: usize = 2;

        let slot = Arc::new(AtomicUsize::new(ACTIVE));
        let phase = Arc::new(AtomicUsize::new(NOT_STARTED));

        let holder = {
            let slot = slot.clone();
            let phase = phase.clone();
            thread::spawn(move || {
                slot.store(RELEASING, Ordering::SeqCst);
                phase.store(RECHECKING, Ordering::SeqCst);
                thread::yield_now(); // stand-in for live-target/revision reads
                phase.store(COMPLETE, Ordering::SeqCst);
                let _ = slot.compare_exchange(RELEASING, IDLE, Ordering::SeqCst, Ordering::SeqCst);
            })
        };

        let observer = {
            let slot = slot.clone();
            let phase = phase.clone();
            thread::spawn(move || {
                let before = phase.load(Ordering::SeqCst);
                let observed_slot = slot.load(Ordering::SeqCst);
                let after = phase.load(Ordering::SeqCst);
                if before == RECHECKING && after == RECHECKING {
                    assert_ne!(
                        observed_slot, IDLE,
                        "holder exposed arbitration IDLE during the recheck interval"
                    );
                }
            })
        };

        holder.join().unwrap();
        observer.join().unwrap();
    });
}

/// Negative control for the historical release protocol documented in
/// `InertiaSlot`: ACTIVE -> IDLE -> recheck -> attempt to become ACTIVE again.
/// Loom must find the exposed-IDLE schedule.
#[test]
#[should_panic]
fn historical_idle_before_recheck_is_detected() {
    loom::model(|| {
        const NOT_STARTED: usize = 0;
        const RECHECKING: usize = 1;
        const COMPLETE: usize = 2;

        let slot = Arc::new(AtomicUsize::new(ACTIVE));
        let phase = Arc::new(AtomicUsize::new(NOT_STARTED));

        let holder = {
            let slot = slot.clone();
            let phase = phase.clone();
            thread::spawn(move || {
                phase.store(RECHECKING, Ordering::SeqCst);
                slot.store(IDLE, Ordering::SeqCst);
                thread::yield_now(); // historical exposed-IDLE recheck window
                phase.store(COMPLETE, Ordering::SeqCst);
                let _ = slot.compare_exchange(IDLE, ACTIVE, Ordering::SeqCst, Ordering::SeqCst);
            })
        };

        let observer = {
            let slot = slot.clone();
            let phase = phase.clone();
            thread::spawn(move || {
                let before = phase.load(Ordering::SeqCst);
                let observed_slot = slot.load(Ordering::SeqCst);
                let after = phase.load(Ordering::SeqCst);
                if before == RECHECKING && after == RECHECKING {
                    assert_ne!(
                        observed_slot, IDLE,
                        "historical release window exposed arbitration IDLE"
                    );
                }
            })
        };

        holder.join().unwrap();
        observer.join().unwrap();
    });
}

/// LC-05 reduced authority model for a mutation whose kick overlaps the release
/// boundary. Owner A is the current convergence holder. If the kick wins while
/// A is RELEASING it must reactivate **A**, not create owner B. If A publishes
/// IDLE first, only then may the kick claim a new owner B.
#[test]
fn racing_releasing_kick_preserves_single_authority() {
    loom::model(|| {
        const IDLE_NONE: usize = 0;
        const ACTIVE_A: usize = 1;
        const RELEASING_A: usize = 2;
        const ACTIVE_B: usize = 3;
        const RELEASE_STARTED: usize = 1;

        let authority = Arc::new(AtomicUsize::new(ACTIVE_A));
        let phase = Arc::new(AtomicUsize::new(0));

        let holder = {
            let authority = authority.clone();
            let phase = phase.clone();
            thread::spawn(move || {
                authority.store(RELEASING_A, Ordering::SeqCst);
                phase.store(RELEASE_STARTED, Ordering::SeqCst);
                thread::yield_now();

                match authority.compare_exchange(
                    RELEASING_A,
                    IDLE_NONE,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                ) {
                    Ok(_) => {}
                    Err(ACTIVE_A) => {
                        // The racing kick reactivated the same logical holder.
                    }
                    Err(other) => {
                        panic!("release transferred authority to unexpected owner {other}")
                    }
                }
            })
        };

        let kick = {
            let authority = authority.clone();
            let phase = phase.clone();
            thread::spawn(move || {
                while phase.load(Ordering::SeqCst) != RELEASE_STARTED {
                    thread::yield_now();
                }

                loop {
                    match authority.load(Ordering::SeqCst) {
                        RELEASING_A => {
                            if authority
                                .compare_exchange(
                                    RELEASING_A,
                                    ACTIVE_A,
                                    Ordering::SeqCst,
                                    Ordering::SeqCst,
                                )
                                .is_ok()
                            {
                                break;
                            }
                        }
                        IDLE_NONE => {
                            if authority
                                .compare_exchange(
                                    IDLE_NONE,
                                    ACTIVE_B,
                                    Ordering::SeqCst,
                                    Ordering::SeqCst,
                                )
                                .is_ok()
                            {
                                break;
                            }
                        }
                        ACTIVE_A | ACTIVE_B => break,
                        other => panic!("invalid modeled authority state {other}"),
                    }
                }
            })
        };

        holder.join().unwrap();
        kick.join().unwrap();
        assert!(matches!(
            authority.load(Ordering::SeqCst),
            ACTIVE_A | ACTIVE_B
        ));
    });
}

/// LC-05 negative control. This deliberately treats a kick that wins against
/// RELEASING as authority transfer to a newly dispatched owner B. The old holder
/// still owns the release attempt, so Loom must find the duplicate-authority
/// interleaving where A observes B instead of its own reactivation.
#[test]
#[should_panic]
fn releasing_kick_that_creates_second_holder_is_detected() {
    loom::model(|| {
        const IDLE_NONE: usize = 0;
        const ACTIVE_A: usize = 1;
        const RELEASING_A: usize = 2;
        const ACTIVE_B: usize = 3;
        const RELEASE_STARTED: usize = 1;

        let authority = Arc::new(AtomicUsize::new(ACTIVE_A));
        let phase = Arc::new(AtomicUsize::new(0));

        let holder = {
            let authority = authority.clone();
            let phase = phase.clone();
            thread::spawn(move || {
                authority.store(RELEASING_A, Ordering::SeqCst);
                phase.store(RELEASE_STARTED, Ordering::SeqCst);
                thread::yield_now();
                match authority.compare_exchange(
                    RELEASING_A,
                    IDLE_NONE,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                ) {
                    Ok(_) | Err(ACTIVE_A) => {}
                    Err(ACTIVE_B) => panic!("racing kick created a second logical holder"),
                    Err(other) => panic!("invalid modeled authority state {other}"),
                }
            })
        };

        let kick = {
            let authority = authority.clone();
            let phase = phase.clone();
            thread::spawn(move || {
                while phase.load(Ordering::SeqCst) != RELEASE_STARTED {
                    thread::yield_now();
                }
                loop {
                    match authority.load(Ordering::SeqCst) {
                        RELEASING_A => {
                            if authority
                                .compare_exchange(
                                    RELEASING_A,
                                    ACTIVE_B,
                                    Ordering::SeqCst,
                                    Ordering::SeqCst,
                                )
                                .is_ok()
                            {
                                break;
                            }
                        }
                        IDLE_NONE => {
                            if authority
                                .compare_exchange(
                                    IDLE_NONE,
                                    ACTIVE_B,
                                    Ordering::SeqCst,
                                    Ordering::SeqCst,
                                )
                                .is_ok()
                            {
                                break;
                            }
                        }
                        ACTIVE_A | ACTIVE_B => break,
                        other => panic!("invalid modeled authority state {other}"),
                    }
                }
            })
        };

        holder.join().unwrap();
        kick.join().unwrap();
    });
}

/// Reduced LC-06 state shared by the ready-history tests.
///
/// `published_epoch` is oracle-only history: it marks the semantic publication
/// point after the target write in the ServiceStore critical section. The
/// production decision never reads it. All protocol decisions use only the same
/// ingredients as production: arbitration state, committed/settled revisions,
/// and the published Fiber state version.
struct ReadyHistoryModel {
    slot: AtomicUsize,
    committed: AtomicUsize,
    settled: AtomicUsize,
    state_version: AtomicUsize,
    published_epoch: AtomicUsize,
}

impl ReadyHistoryModel {
    fn new() -> Self {
        Self {
            slot: AtomicUsize::new(IDLE),
            committed: AtomicUsize::new(0),
            settled: AtomicUsize::new(0),
            state_version: AtomicUsize::new(0),
            published_epoch: AtomicUsize::new(0),
        }
    }

    fn has_committed_recheck(&self) -> bool {
        self.committed.load(Ordering::SeqCst) != self.settled.load(Ordering::SeqCst)
    }

    /// One already-proved Service semantic commit. LC-03 separately models the
    /// target-inspection coverage behind this abstraction; LC-06 only needs to
    /// know that version 1 is durably committed before executor-dependent kick.
    fn commit_one_mutation(&self) {
        self.committed.store(1, Ordering::SeqCst);
        // Oracle-only history marker. Production ready never reads it.
        self.published_epoch.store(1, Ordering::SeqCst);
    }

    /// One finite successful holder pass for the single modeled mutation. LC-05
    /// separately proves release/kick authority races; here a winner publishes
    /// state before acknowledging the revision and releasing arbitration.
    fn settle_claimed_mutation(&self) {
        self.state_version.store(1, Ordering::SeqCst);
        self.slot.store(RELEASING, Ordering::SeqCst);
        self.settled.store(1, Ordering::SeqCst);
        self.slot
            .compare_exchange(RELEASING, IDLE, Ordering::SeqCst, Ordering::SeqCst)
            .expect("the finite LC-06 model has no second release claimant");
    }

    /// Best-effort acceleration for the one modeled obligation. `false` means
    /// another modeled holder already owns/release-tests the slot, so a real
    /// ready call would wait and retry rather than make an acceptance decision.
    fn kick_once(&self) -> bool {
        if !self.has_committed_recheck() {
            return true;
        }
        if self
            .slot
            .compare_exchange(IDLE, ACTIVE, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            self.settle_claimed_mutation();
            true
        } else {
            false
        }
    }

    /// Phase-1 model of production ready's non-blocking acceptance decision.
    /// Busy/releasing schedules return `None`; full waiting and lost-wakeup
    /// progress are intentionally deferred to Phase 2.
    fn try_ready_acceptance(&self) -> Option<usize> {
        if self.has_committed_recheck() && !self.kick_once() {
            return None;
        }
        if self.slot.load(Ordering::SeqCst) != IDLE {
            return None;
        }

        let observed_state = self.state_version.load(Ordering::SeqCst);
        (self.slot.load(Ordering::SeqCst) == IDLE && !self.has_committed_recheck())
            .then_some(observed_state)
    }

    fn mutation_kick_once(&self) {
        if !self.has_committed_recheck() {
            return;
        }
        if self
            .slot
            .compare_exchange(IDLE, ACTIVE, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            self.settle_claimed_mutation();
        }
    }
}

/// LC-06 history model. A returned old state is legal only when the mutation had
/// not reached its semantic publication point before this ready invocation: the
/// call may then linearize before the racing mutation even if it physically
/// returns later. If publication already preceded invocation, `ready()` must
/// drive/await the obligation and return the converged state instead.
#[test]
fn ready_return_has_a_quiescent_linearization_point() {
    lc06_model(|| {
        let model = Arc::new(ReadyHistoryModel::new());

        let mutation = {
            let model = model.clone();
            thread::spawn(move || {
                model.commit_one_mutation();
                // Preserve the real commit-to-kick window: revision/visibility
                // are durable truth before executor-dependent acceleration.
                thread::yield_now();
                model.mutation_kick_once();
            })
        };

        let ready = {
            let model = model.clone();
            thread::spawn(move || {
                // Oracle-only invocation history. Version 0 means there was a
                // valid version-0 quiescent point at invocation; version 1 means
                // the mutation's semantic publication already preceded it.
                let published_at_invocation = model.published_epoch.load(Ordering::SeqCst);
                let Some(returned) = model.try_ready_acceptance() else {
                    // This schedule reached a busy/releasing point. Production
                    // ready would wait/retry; Phase 2 models that progress.
                    return;
                };

                match returned {
                    0 => assert_eq!(
                        published_at_invocation, 0,
                        "ready returned pre-mutation state although publication preceded invocation"
                    ),
                    1 => {
                        // `try_ready_acceptance` accepted version 1 only after its own
                        // IDLE + no-pending checks. Do not inspect current state
                        // again here: a later mutation may legally start after
                        // that decision and before this oracle assertion runs.
                    }
                    other => panic!("invalid modeled Fiber state version {other}"),
                }
            })
        };

        mutation.join().unwrap();
        ready.join().unwrap();
    });
}

/// LC-06 negative control. This deliberately reduces ready to `wait until IDLE;
/// read state; return`, omitting both durable-obligation driving and the final
/// committed-vs-settled recheck. The mutation is forced to publish before the
/// invocation while its kick is still delayed. Loom must then expose the stale
/// version-0 return with no valid quiescence point inside the invocation.
#[test]
#[should_panic]
fn ready_that_treats_idle_as_quiescent_is_detected() {
    lc06_model(|| {
        let model = Arc::new(ReadyHistoryModel::new());

        let mutation = {
            let model = model.clone();
            thread::spawn(move || {
                model.commit_one_mutation();
                thread::yield_now();
                model.mutation_kick_once();
            })
        };

        let ready = {
            let model = model.clone();
            thread::spawn(move || {
                // Force the bad case: semantic publication has committed, but
                // the executor-dependent kick may not have run yet.
                while model.published_epoch.load(Ordering::SeqCst) == 0 {
                    thread::yield_now();
                }
                let published_at_invocation = model.published_epoch.load(Ordering::SeqCst);

                while model.slot.load(Ordering::SeqCst) != IDLE {
                    thread::yield_now();
                }
                let returned = model.state_version.load(Ordering::SeqCst);

                if returned == 0 && published_at_invocation == 1 {
                    panic!("stale ready return has no quiescent linearization point");
                }
            })
        };

        mutation.join().unwrap();
        ready.join().unwrap();
    });
}
