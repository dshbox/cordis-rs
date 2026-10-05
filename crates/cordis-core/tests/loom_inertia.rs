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
#[should_panic(expected = "historical publication-to-revision window reproduced")]
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
#[should_panic(expected = "premature acknowledgement reproduced")]
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
#[should_panic(expected = "historical release window exposed arbitration IDLE")]
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
#[should_panic(expected = "racing kick created a second logical holder")]
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

/// Modeled Fiber states for the LC-06 ready history. `STATE_V0` and `STATE_V1`
/// stand for stable `FiberState::Active` before and after the single modeled
/// Service mutation. `STATE_LOADING` stands for the transient lifecycle states
/// (`Loading` / `Unloading`) that a holder publishes only while it owns the
/// slot.
const STATE_V0: usize = 0;
const STATE_V1: usize = 1;
const STATE_LOADING: usize = 2;
/// Radix that packs `(sequence, state)` into one state-publication word.
const STATE_RADIX: usize = 4;

/// One state publication: the published state and the total publication
/// sequence that identifies it. A restart republishes the same stable state,
/// so only the sequence tells two publications of `STATE_V0` apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StatePublication {
    state: usize,
    sequence: usize,
}

impl StatePublication {
    fn unpack(word: usize) -> Self {
        Self {
            state: word % STATE_RADIX,
            sequence: word / STATE_RADIX,
        }
    }

    fn pack(self) -> usize {
        self.sequence * STATE_RADIX + self.state
    }

    /// Oracle-only bitset member for this publication.
    fn bit(self) -> usize {
        1 << self.sequence
    }
}

/// Whether the modeled ready keeps production's publication-sequence recheck.
/// `Omitted` exists only for the negative control.
#[derive(Clone, Copy)]
enum SequenceRecheck {
    Production,
    Omitted,
}

/// Reduced LC-06 state shared by the ready-history tests.
///
/// Production mapping (paths under `crates/cordis-core/src/fiber/`):
///
/// - `slot`, `committed`, `settled`: `InertiaSlot`'s arbitration word and
///   `recheck_committed` / `recheck_settled` revisions (`inertia.rs`).
/// - `state`: the Fiber's `StatePublication` cell (`mod.rs`; inner fields at
///   L124-L131). Production keeps `current` and the total `sequence` under
///   one mutex: `publish` (L160-L179) advances both in one critical section
///   and `observe` (L186-L189) reads them as one pair. The model packs the
///   pair into one SeqCst word, so `publish` is one read-modify-write on it and
///   `observe` is one load. That keeps the pair atomic as the mutex does, and
///   it adds no synchronization edge that the production lock does not
///   already provide.
///
/// Oracle-only history, which no protocol decision reads:
///
/// - `published_epoch` marks the semantic publication point after the target
///   write in the ServiceStore critical section.
/// - `quiescent_states` is a bitset over state-publication sequences. See
///   `assert_ready_answer_had_quiescent_point`.
///
/// All protocol decisions use only the same ingredients as production:
/// arbitration state, committed/settled revisions, and the published Fiber
/// state with its publication sequence.
struct ReadyHistoryModel {
    slot: AtomicUsize,
    committed: AtomicUsize,
    settled: AtomicUsize,
    state: AtomicUsize,
    published_epoch: AtomicUsize,
    quiescent_states: AtomicUsize,
}

impl ReadyHistoryModel {
    fn new() -> Self {
        let initial = StatePublication {
            state: STATE_V0,
            sequence: 0,
        };
        Self {
            slot: AtomicUsize::new(IDLE),
            committed: AtomicUsize::new(0),
            settled: AtomicUsize::new(0),
            state: AtomicUsize::new(initial.pack()),
            published_epoch: AtomicUsize::new(0),
            // The Fiber starts quiescent with its initial publication current.
            quiescent_states: AtomicUsize::new(initial.bit()),
        }
    }

    fn has_committed_recheck(&self) -> bool {
        self.committed.load(Ordering::SeqCst) != self.settled.load(Ordering::SeqCst)
    }

    /// `StatePublication::publish` (`mod.rs` L160-L179), reached through
    /// `Fiber::transition` (L597). Like production, publishing the current
    /// state is a no-op that leaves the sequence alone; this happens when a
    /// second holder pass re-converges an already-converged target. Only the
    /// current slot holder publishes in these models, so no second publisher
    /// can interleave with this read-modify-write, matching the production
    /// critical section.
    fn publish(&self, next: usize) -> Option<StatePublication> {
        self.state
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |word| {
                let current = StatePublication::unpack(word);
                (current.state != next).then(|| {
                    StatePublication {
                        state: next,
                        sequence: current.sequence + 1,
                    }
                    .pack()
                })
            })
            .ok()
            .map(|previous| StatePublication {
                state: next,
                sequence: StatePublication::unpack(previous).sequence + 1,
            })
    }

    /// `StatePublication::observe` (`mod.rs` L186-L189).
    fn observe(&self) -> StatePublication {
        StatePublication::unpack(self.state.load(Ordering::SeqCst))
    }

    /// `StatePublication::sequence` (`mod.rs` L191-L193).
    fn sequence(&self) -> usize {
        self.observe().sequence
    }

    /// Clean-probe tail of `InertiaSlot::exit_recheck` (`inertia.rs`
    /// L296-L342): publish RELEASING (L304), acknowledge the inspected revision
    /// (L322), then CAS RELEASING -> IDLE (L323-L328). LC-03 and LC-05 model
    /// the drift and racing-kick arms that these finite histories never take.
    fn release_after_clean_probe(&self, revision: usize) {
        self.slot.store(RELEASING, Ordering::SeqCst);
        self.settled.store(revision, Ordering::SeqCst);
        self.slot
            .compare_exchange(RELEASING, IDLE, Ordering::SeqCst, Ordering::SeqCst)
            .expect("the finite LC-06 model has no second release claimant");
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
    /// state before acknowledging the revision and releasing arbitration. This
    /// pass publishes only its final state; the restart history below carries
    /// the transient publication.
    fn settle_claimed_mutation(&self) {
        self.publish(STATE_V1);
        self.release_after_clean_probe(1);
    }

    /// One complete restart-like holder pass for the LC-06 sequence history.
    ///
    /// Production: `FiberHandle::restart` (`mod.rs` L1726) enters
    /// `InertiaSlot::restart_pass` (`inertia.rs` L596-L643), which claims the
    /// slot (L601; `claim` CASes IDLE -> ACTIVE at L206-L225) and reads the
    /// committed revision (L615). `restart_committed` (L649-L671) then runs
    /// `settle_once`, which publishes `Unloading` (`mod.rs` L908), `Loading`
    /// (L947) and, after a successful apply, `Active` (L966). A clean
    /// `exit_recheck` releases the slot.
    ///
    /// The model collapses `Unloading` and `Loading` into one transient
    /// publication. Both are published only while the slot is held, and one
    /// transient publication already opens the window that the sequence
    /// recheck guards. The restart re-applies the same target, so its final
    /// state equals the state it replaced.
    fn restart_pass(&self) {
        self.slot
            .compare_exchange(IDLE, ACTIVE, Ordering::SeqCst, Ordering::SeqCst)
            .expect("no other modeled actor claims the slot in the restart history");
        let revision = self.committed.load(Ordering::SeqCst);
        self.publish(STATE_LOADING);
        let last = self
            .publish(STATE_V0)
            .expect("the restart's stable state replaces its transient state");
        // Oracle-only: `last` becomes current at a quiescent point at the
        // releasing CAS. Recording it before the release can only make the
        // recorded quiescent window start earlier than the real one.
        self.quiescent_states.fetch_or(last.bit(), Ordering::SeqCst);
        self.release_after_clean_probe(revision);
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

    /// Phase-1 model of production ready's non-blocking acceptance decision,
    /// in the order of one `FiberHandle::ready` loop iteration (`mod.rs`
    /// L1613-L1656):
    ///
    /// 1. drive an outstanding committed recheck (L1617-L1621);
    /// 2. wait for arbitration IDLE (L1622);
    /// 3. observe the state together with its sequence (L1625);
    /// 4. idle / committed-recheck test (L1628-L1630);
    /// 5. publication-sequence recheck (L1635-L1637);
    /// 6. answer (L1638-L1655).
    ///
    /// Where production would wait or go around the loop (steps 1, 2, 4 and
    /// 5), the model returns `None`; full waiting, retry and lost-wakeup
    /// progress are intentionally deferred to Phase 2. The answer is returned
    /// without production's `unreachable!` on a transient state (L1652-L1654),
    /// so the test oracle judges it.
    fn try_ready_acceptance(&self, recheck: SequenceRecheck) -> Option<StatePublication> {
        if self.has_committed_recheck() && !self.kick_once() {
            return None;
        }
        if self.slot.load(Ordering::SeqCst) != IDLE {
            return None;
        }

        let observed = self.observe();
        if self.slot.load(Ordering::SeqCst) != IDLE || self.has_committed_recheck() {
            return None;
        }
        if matches!(recheck, SequenceRecheck::Production) && self.sequence() != observed.sequence {
            return None;
        }
        Some(observed)
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
                let Some(returned) = model.try_ready_acceptance(SequenceRecheck::Production) else {
                    // This schedule reached a busy/releasing point. Production
                    // ready would wait/retry; Phase 2 models that progress.
                    return;
                };

                match returned.state {
                    STATE_V0 => assert_eq!(
                        published_at_invocation, 0,
                        "ready returned pre-mutation state although publication preceded invocation"
                    ),
                    STATE_V1 => {
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
#[should_panic(expected = "stale ready return has no quiescent linearization point")]
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
                let returned = model.observe().state;

                if returned == STATE_V0 && published_at_invocation == 1 {
                    panic!("stale ready return has no quiescent linearization point");
                }
            })
        };

        mutation.join().unwrap();
        ready.join().unwrap();
    });
}

/// LC-06 oracle for the publication-sequence history.
///
/// A ready answer is stale unless the answered publication was current at some
/// point inside the invocation `[I, R]` at which the Fiber was semantically
/// quiescent (`slot == IDLE && committed == settled`). In the restart history
/// no Service mutation commits, so quiescence begins only at model start or at
/// a holder's releasing CAS, and ends at the next claim. Each publication is
/// therefore current while quiescent during at most one window `[q, e)`: `q` is
/// model start (initial publication) or the release of the pass that published
/// it, and `e` is no later than its supersession by the next publication. A
/// transient publication has no such window, because its holder publishes a
/// stable state before releasing. The answer is legal exactly when its window
/// meets the invocation: `q <= R` and `e > I`.
///
/// The history over-approximates the window, so the oracle may accept more
/// than the exact window but never rejects a legal answer:
///
/// - `q <= R`: the restart pass records `q` in `quiescent_states` before its
///   releasing CAS, which is no later than the real start. The ready actor
///   reads that bitset at return.
/// - `e > I`: supersession is exact, because the ready actor reads the current
///   publication sequence at invocation. The answer was not yet superseded at
///   `I` when its sequence is no lower than that one.
///
/// A transient answer is checked first, so it fails with its own message.
fn assert_ready_answer_had_quiescent_point(
    answer: StatePublication,
    sequence_at_invocation: usize,
    quiescent_at_return: usize,
) {
    assert_ne!(
        answer.state, STATE_LOADING,
        "stale ready answer: transient state escaped ready"
    );
    assert!(
        quiescent_at_return & answer.bit() != 0 && answer.sequence >= sequence_at_invocation,
        "stale ready answer: publication {answer:?} was not current at any quiescent point inside the invocation"
    );
}

/// The restart history shared by the LC-06 sequence model and its negative
/// control. One restart-like pass (claim -> publish transient -> publish
/// stable -> release) races one ready actor. The whole pass can run between
/// ready's state observation and its idle recheck. The idle recheck then passes,
/// and only the publication-sequence recheck tells the stale observation from a
/// current one.
///
/// Layer-C counterpart: the real Tokio regression
/// `fiber::tests::ready_transient_state_racing_complete_restart_never_panics`
/// (`mod.rs` L1868-L1932) drives the same five steps with probes.
fn ready_restart_history(recheck: SequenceRecheck) {
    let model = Arc::new(ReadyHistoryModel::new());

    let restart = {
        let model = model.clone();
        thread::spawn(move || model.restart_pass())
    };

    let ready = {
        let model = model.clone();
        thread::spawn(move || {
            // Oracle-only invocation and return history.
            let sequence_at_invocation = model.sequence();
            let Some(answer) = model.try_ready_acceptance(recheck) else {
                // Busy, releasing or superseded observation. Production ready
                // would wait/retry; Phase 2 models that progress.
                return;
            };
            let quiescent_at_return = model.quiescent_states.load(Ordering::SeqCst);
            assert_ready_answer_had_quiescent_point(
                answer,
                sequence_at_invocation,
                quiescent_at_return,
            );
        })
    };

    restart.join().unwrap();
    ready.join().unwrap();
}

/// LC-06 publication-sequence model (#248). With production's sequence
/// recheck, every ready answer was current at a quiescent point inside its
/// invocation, even when a complete restart pass lands between the state
/// observation and the idle recheck.
#[test]
fn ready_answer_survives_a_complete_racing_restart_pass() {
    lc06_model(|| ready_restart_history(SequenceRecheck::Production));
}

/// LC-06 negative control for the publication-sequence recheck. Without it,
/// Loom finds the schedule from the real Tokio regression: ready observes an
/// idle slot, the restart claims it and publishes the transient state, ready
/// reads that state, the restart publishes its stable state and releases, and
/// ready's idle recheck passes. Ready then answers the transient state.
#[test]
#[should_panic(expected = "stale ready answer: transient state escaped ready")]
fn ready_without_sequence_recheck_is_detected() {
    lc06_model(|| ready_restart_history(SequenceRecheck::Omitted));
}

/// Phase-1 scenario 1: two independent kicks compete from arbitration IDLE.
/// Neither actor releases the claimed slot inside this reduced scenario, so
/// exactly one IDLE -> ACTIVE CAS may acquire convergence authority.
#[test]
fn two_idle_kickers_have_one_claim_winner() {
    loom::model(|| {
        let slot = Arc::new(AtomicUsize::new(IDLE));
        let winners = Arc::new(AtomicUsize::new(0));

        let spawn_kicker = |slot: Arc<AtomicUsize>, winners: Arc<AtomicUsize>| {
            thread::spawn(move || {
                if slot
                    .compare_exchange(IDLE, ACTIVE, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    winners.fetch_add(1, Ordering::SeqCst);
                }
            })
        };

        let a = spawn_kicker(slot.clone(), winners.clone());
        let b = spawn_kicker(slot.clone(), winners.clone());
        a.join().unwrap();
        b.join().unwrap();

        assert_eq!(slot.load(Ordering::SeqCst), ACTIVE);
        assert_eq!(
            winners.load(Ordering::SeqCst),
            1,
            "two IDLE kickers acquired convergence authority"
        );
    });
}

/// Negative control for the two-kicker claim. Replacing the atomic claim with
/// load -> yield -> store lets both actors decide they won from the same IDLE
/// observation. Loom must find that duplicate-authority history.
#[test]
#[should_panic(expected = "load/store claim admitted two logical holders")]
fn load_then_store_idle_claim_is_detected() {
    loom::model(|| {
        let slot = Arc::new(AtomicUsize::new(IDLE));
        let winners = Arc::new(AtomicUsize::new(0));

        let spawn_bad_kicker = |slot: Arc<AtomicUsize>, winners: Arc<AtomicUsize>| {
            thread::spawn(move || {
                if slot.load(Ordering::SeqCst) == IDLE {
                    thread::yield_now();
                    slot.store(ACTIVE, Ordering::SeqCst);
                    winners.fetch_add(1, Ordering::SeqCst);
                }
            })
        };

        let a = spawn_bad_kicker(slot.clone(), winners.clone());
        let b = spawn_bad_kicker(slot.clone(), winners.clone());
        a.join().unwrap();
        b.join().unwrap();

        assert_eq!(
            winners.load(Ordering::SeqCst),
            1,
            "load/store claim admitted two logical holders"
        );
    });
}

/// Phase-1 scenario 5 / LC-04 finite-drain model. A semantic mutation may commit
/// while no executor exists, leaving arbitration IDLE with committed != settled.
/// A later legitimate driver must still find that obligation, claim the slot,
/// converge the published state, acknowledge the revision, and return to
/// semantic quiescence.
#[test]
fn off_runtime_commit_is_preserved_for_later_drive() {
    loom::model(|| {
        let slot = Arc::new(AtomicUsize::new(IDLE));
        let committed = Arc::new(AtomicUsize::new(0));
        let settled = Arc::new(AtomicUsize::new(0));
        let state_version = Arc::new(AtomicUsize::new(0));

        // No-executor semantic commit: durable truth advances, acceleration does
        // not. IDLE therefore means only "no holder", not quiescence.
        committed.store(1, Ordering::SeqCst);
        assert_eq!(slot.load(Ordering::SeqCst), IDLE);
        assert_ne!(
            committed.load(Ordering::SeqCst),
            settled.load(Ordering::SeqCst)
        );

        let driver = {
            let slot = slot.clone();
            let committed = committed.clone();
            let settled = settled.clone();
            let state_version = state_version.clone();
            thread::spawn(move || {
                if committed.load(Ordering::SeqCst) == settled.load(Ordering::SeqCst) {
                    return;
                }
                slot.compare_exchange(IDLE, ACTIVE, Ordering::SeqCst, Ordering::SeqCst)
                    .expect("later legitimate driver claims preserved obligation");
                state_version.store(1, Ordering::SeqCst);
                slot.store(RELEASING, Ordering::SeqCst);
                settled.store(committed.load(Ordering::SeqCst), Ordering::SeqCst);
                slot.compare_exchange(RELEASING, IDLE, Ordering::SeqCst, Ordering::SeqCst)
                    .expect("finite drain releases the claimed slot");
            })
        };
        driver.join().unwrap();

        assert_eq!(state_version.load(Ordering::SeqCst), 1);
        assert_eq!(slot.load(Ordering::SeqCst), IDLE);
        assert_eq!(
            committed.load(Ordering::SeqCst),
            settled.load(Ordering::SeqCst)
        );
    });
}

/// LC-04 negative control. This deliberately treats "no executor" as if the
/// recheck had already settled. The later driver then sees no obligation and the
/// semantic state remains stale even though the committed revision advanced.
#[test]
#[should_panic(expected = "executor absence erased a durable convergence obligation")]
fn off_runtime_commit_must_not_be_consumed_without_drive() {
    loom::model(|| {
        let slot = Arc::new(AtomicUsize::new(IDLE));
        let committed = Arc::new(AtomicUsize::new(1));
        let settled = Arc::new(AtomicUsize::new(0));
        let state_version = Arc::new(AtomicUsize::new(0));

        // Historical-bad interpretation: executor absence consumes protocol
        // truth instead of merely skipping the best-effort kick.
        settled.store(committed.load(Ordering::SeqCst), Ordering::SeqCst);

        let driver = {
            let committed = committed.clone();
            let settled = settled.clone();
            let state_version = state_version.clone();
            thread::spawn(move || {
                if committed.load(Ordering::SeqCst) != settled.load(Ordering::SeqCst) {
                    state_version.store(1, Ordering::SeqCst);
                }
            })
        };
        driver.join().unwrap();

        assert_eq!(
            state_version.load(Ordering::SeqCst),
            1,
            "executor absence erased a durable convergence obligation"
        );
        assert_eq!(slot.load(Ordering::SeqCst), IDLE);
    });
}

/// Phase-1 scenario 4: two semantic mutations race around one holder's target
/// inspection and acknowledgement. Any acknowledgement the holder actually
/// publishes must be covered by its inspected target version; after both
/// mutations stop, one finite final drain reaches revision 2.
///
/// The holder returns each intermediate acknowledgement as an oracle event
/// `(inspected, acknowledged_revision)` through its join handle. The protocol
/// never reads these events, and they add no synchronization beyond the join
/// the model already performs, so the oracle introduces no happens-before edge
/// absent from production. The events are checked after the join and before
/// the final drain, which overwrites `settled` but cannot erase them.
#[test]
fn two_mutations_preserve_revision_inspection_coverage() {
    two_mutator_model(|inspected, observed_revision| inspected >= observed_revision);
}

/// Scenario 4 negative control. This deliberately removes the holder's
/// covering-inspection guard: it acknowledges whatever revision it observes
/// after its target inspection, even when a mutation committed in between.
/// The final drain still reaches revision 2, so only the intermediate
/// acknowledgement oracle can fail. Loom must find that interleaving within the
/// same declared range as the positive model.
#[test]
#[should_panic(expected = "intermediate acknowledgement outran its target inspection")]
fn unguarded_intermediate_acknowledgement_is_detected() {
    two_mutator_model(|_inspected, _observed_revision| true);
}

/// Shared declared range and history for scenario 4. `acknowledgement_permitted`
/// is the holder's decision `(inspected, observed_revision) -> acknowledge?`.
fn two_mutator_model(acknowledgement_permitted: fn(usize, usize) -> bool) {
    let mut builder = loom::model::Builder::new();
    builder.max_threads = 4; // main + two mutators + holder
    builder.max_branches = 48;
    // This is the first larger Phase-1 scenario. Keep its declared range
    // explicit and bounded; the smaller core models remain exhaustive without
    // a preemption bound.
    let preemption_bound = std::env::var("CORDIS_LOOM_INERTIA_PREEMPTION_BOUND")
        .ok()
        .map(|value| {
            value
                .parse()
                .expect("CORDIS_LOOM_INERTIA_PREEMPTION_BOUND must be an integer")
        })
        .unwrap_or(3);
    builder.preemption_bound = Some(preemption_bound);
    builder.max_permutations = None;
    builder.max_duration = None;
    eprintln!(
        "CORDIS_LOOM_RANGE actors=4 max_threads=4 max_branches=48 preemption_bound={preemption_bound} max_permutations=none max_duration=none"
    );
    builder.check(move || {
        let committed = Arc::new(AtomicUsize::new(0));
        let settled = Arc::new(AtomicUsize::new(0));
        let target_version = Arc::new(Mutex::new(0usize));

        let spawn_mutator = |committed: Arc<AtomicUsize>, target_version: Arc<Mutex<usize>>| {
            thread::spawn(move || {
                let mut target = target_version.lock().unwrap();
                let revision = committed.fetch_add(1, Ordering::SeqCst) + 1;
                // ADR 0031 semantic commit keeps target readers excluded
                // until the corresponding revision and visibility agree.
                *target = revision;
            })
        };

        let a = spawn_mutator(committed.clone(), target_version.clone());
        let b = spawn_mutator(committed.clone(), target_version.clone());

        let holder = {
            let committed = committed.clone();
            let settled = settled.clone();
            let target_version = target_version.clone();
            thread::spawn(move || {
                // Oracle-only record, local to the holder until join.
                let mut acknowledgements = Vec::new();
                let inspected = *target_version.lock().unwrap();
                thread::yield_now();
                let observed_revision = committed.load(Ordering::SeqCst);

                // If a newer commit landed after the target inspection, the
                // production holder must re-inspect rather than acknowledge it.
                if acknowledgement_permitted(inspected, observed_revision) {
                    settled.store(observed_revision, Ordering::SeqCst);
                    acknowledgements.push((inspected, observed_revision));
                }
                acknowledgements
            })
        };

        a.join().unwrap();
        b.join().unwrap();
        let acknowledgements = holder.join().unwrap();

        // Intermediate acknowledgement oracle, checked before the final drain
        // overwrites `settled`.
        for &(inspected, acknowledged_revision) in &acknowledgements {
            assert!(
                acknowledged_revision <= inspected,
                "intermediate acknowledgement outran its target inspection: revision {acknowledged_revision}, target version {inspected}"
            );
        }
        assert_eq!(
            settled.load(Ordering::SeqCst),
            acknowledgements
                .last()
                .map_or(0, |&(_, acknowledged_revision)| acknowledged_revision),
            "oracle events diverged from the holder's published acknowledgement"
        );

        // Explicit progress assumption for LC-07's finite drain: mutations have
        // stopped and a legitimate holder gets one final covering inspection.
        let inspected = *target_version.lock().unwrap();
        let observed_revision = committed.load(Ordering::SeqCst);
        assert!(
            inspected >= observed_revision,
            "final drain inspected target {inspected} behind revision {observed_revision}"
        );
        settled.store(observed_revision, Ordering::SeqCst);

        assert_eq!(observed_revision, 2);
        assert_eq!(inspected, 2);
        assert_eq!(settled.load(Ordering::SeqCst), 2);
    });
    eprintln!("CORDIS_LOOM_RESULT declared_range_completed preemption_bound={preemption_bound}");
}
