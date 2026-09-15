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
