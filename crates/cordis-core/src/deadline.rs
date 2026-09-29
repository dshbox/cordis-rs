//! The std-clock deadline arm, written once. Core carries no Tokio time
//! (ADR 0036: Tokio-time integration belongs to `cordis-timer`), and core
//! waits must work off-runtime and on runtimes built without a time driver,
//! so a deadline is a oneshot that a std scheduler thread fires, raced against
//! the wait via `select`.
//!
//! One lazily started process-wide scheduler thread serves every armed
//! deadline, so pending waits cost one ordered-map entry each rather than one
//! OS thread each. Arming is fallible: the scheduler thread may be refused by
//! the OS the first time it is needed, and that refusal is reported to the
//! caller instead of panicking. A later arm retries the spawn.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use tokio::sync::oneshot;

/// Exact identity of one armed deadline: earliest first, arming order breaks ties.
type DeadlineKey = (Instant, u64);

/// The process refused to start the deadline scheduler thread.
#[derive(Debug)]
pub(crate) struct DeadlineUnavailable;

struct Schedule {
    armed: BTreeMap<DeadlineKey, oneshot::Sender<()>>,
    next_id: u64,
    worker_running: bool,
}

/// Ordered deadlines plus the single worker that fires them. Arming inserts
/// under the lock and wakes the worker only when the new entry is the earliest;
/// the worker recomputes its next wake from the map under the same lock before
/// every wait, so neither an insertion nor a cancellation can be lost. Due
/// senders are fired after the lock is released, so waking a task never runs
/// under scheduler synchronization, and each wake is contained on its own: a
/// caller's panicking `Waker` loses only that caller's wake. The worker also
/// survives any other unexpected panic, and a lost worker is replaced at once
/// whenever deadlines are still armed.
pub(crate) struct DeadlineScheduler {
    schedule: Mutex<Schedule>,
    changed: Condvar,
    #[cfg(test)]
    refuse_spawn: std::sync::atomic::AtomicBool,
    /// Test hook: the serving loop panics once outside delivery containment.
    #[cfg(test)]
    panic_serving_once: std::sync::atomic::AtomicBool,
    /// Test hook: the worker thread exits once as if it had been lost.
    #[cfg(test)]
    exit_worker_once: std::sync::atomic::AtomicBool,
}

static SCHEDULER: DeadlineScheduler = DeadlineScheduler::new();

impl DeadlineScheduler {
    const fn new() -> Self {
        Self {
            schedule: Mutex::new(Schedule {
                armed: BTreeMap::new(),
                next_id: 0,
                worker_running: false,
            }),
            changed: Condvar::new(),
            #[cfg(test)]
            refuse_spawn: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            panic_serving_once: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            exit_worker_once: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Arm `timeout` from now. A deadline beyond the representable monotonic
    /// clock never elapses and needs no scheduler; one already due fires
    /// immediately. Otherwise the entry is inserted only once a worker exists,
    /// so a refused spawn never drops a sender (which would read as elapsed).
    fn arm(&'static self, timeout: Duration) -> std::result::Result<Watchdog, DeadlineUnavailable> {
        let (sender, receiver) = oneshot::channel();
        let now = Instant::now();
        let Some(deadline) = now.checked_add(timeout) else {
            return Ok(Watchdog {
                receiver,
                slot: Slot::Never(sender),
            });
        };
        if deadline <= now {
            let _ = sender.send(());
            return Ok(Watchdog {
                receiver,
                slot: Slot::Fired,
            });
        }

        let key = self.insert(deadline, sender)?;
        Ok(Watchdog {
            receiver,
            slot: Slot::Armed {
                scheduler: self,
                key,
            },
        })
    }

    /// Schedule one sender, starting the worker first if none is running.
    fn insert(
        &'static self,
        deadline: Instant,
        sender: oneshot::Sender<()>,
    ) -> std::result::Result<DeadlineKey, DeadlineUnavailable> {
        let (key, earliest) = {
            let mut schedule = self.schedule.lock();
            if !schedule.worker_running {
                self.spawn_worker()?;
                schedule.worker_running = true;
            }
            let key = (deadline, schedule.next_id);
            schedule.next_id = schedule
                .next_id
                .checked_add(1)
                .expect("deadline identity space exhausted");
            let earliest = schedule
                .armed
                .first_key_value()
                .is_none_or(|(first, _)| key < *first);
            schedule.armed.insert(key, sender);
            (key, earliest)
        };
        if earliest {
            self.changed.notify_one();
        }
        Ok(key)
    }

    /// Remove a cancelled deadline exactly. The worker may still wake at the
    /// removed instant; it then finds nothing due and recomputes.
    fn disarm(&self, key: &DeadlineKey) {
        let removed = self.schedule.lock().armed.remove(key);
        // Released outside the lock; the owning receiver is already closed.
        drop(removed);
    }

    fn spawn_worker(&'static self) -> std::result::Result<(), DeadlineUnavailable> {
        #[cfg(test)]
        if self.refuse_spawn.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(DeadlineUnavailable);
        }
        std::thread::Builder::new()
            .name("cordis-deadline".into())
            .spawn(move || self.worker_main())
            .map(drop)
            .map_err(|_| DeadlineUnavailable)
    }

    /// The worker thread body. Delivery wakes are contained one by one inside
    /// [`Self::serve`]; any other unexpected panic is contained here and
    /// serving resumes on the same thread, so armed deadlines keep firing.
    fn worker_main(&'static self) {
        // Last resort if the thread exits anyway: hand every still-armed
        // entry to a replacement now instead of waiting for a future arm.
        struct Retire(&'static DeadlineScheduler);
        impl Drop for Retire {
            fn drop(&mut self) {
                let mut schedule = self.0.schedule.lock();
                schedule.worker_running = false;
                // Spawning under the lock is the same choreography as `arm`:
                // the replacement blocks on this lock until it is released.
                // If the OS refuses, the entries stay armed (never dropped as
                // elapsed) and the next arm retries the spawn.
                if !schedule.armed.is_empty() && self.0.spawn_worker().is_ok() {
                    schedule.worker_running = true;
                }
            }
        }
        let _retire = Retire(self);

        loop {
            let mut exited = false;
            crate::contained::contain("deadline scheduler", None, || {
                self.serve();
                exited = true;
            });
            if exited {
                return;
            }
        }
    }

    /// Fire due deadlines until the process ends. Returns only through the
    /// test-only worker-exit hook.
    fn serve(&self) {
        let mut due = Vec::new();
        let mut schedule = self.schedule.lock();
        loop {
            #[cfg(test)]
            {
                use std::sync::atomic::Ordering;
                if self.exit_worker_once.swap(false, Ordering::SeqCst) {
                    return;
                }
                if self.panic_serving_once.swap(false, Ordering::SeqCst) {
                    panic!("injected deadline scheduler fault");
                }
            }
            let now = Instant::now();
            while let Some(entry) = schedule.armed.first_entry() {
                if entry.key().0 > now {
                    break;
                }
                due.push(entry.remove());
            }
            if !due.is_empty() {
                drop(schedule);
                for sender in due.drain(..) {
                    // Sending wakes the waiting task through its caller-supplied
                    // Waker, which may panic. Contain each wake on its own so one
                    // panicking Waker cannot stop delivery to the others.
                    crate::contained::contain("wait_state deadline wake", None, || {
                        let _ = sender.send(());
                    });
                }
                schedule = self.schedule.lock();
                continue;
            }
            match schedule.armed.first_key_value() {
                Some((&(deadline, _), _)) => {
                    self.changed.wait_until(&mut schedule, deadline);
                }
                None => self.changed.wait(&mut schedule),
            }
        }
    }

    #[cfg(test)]
    fn armed_len(&self) -> usize {
        self.schedule.lock().armed.len()
    }
}

enum Slot {
    /// Registered with the scheduler; removed exactly on drop.
    Armed {
        scheduler: &'static DeadlineScheduler,
        key: DeadlineKey,
    },
    /// Unrepresentable deadline: holding the sender keeps the arm pending.
    Never(#[allow(dead_code)] oneshot::Sender<()>),
    /// Already due when armed.
    Fired,
}

/// Deadline arm: resolves once its deadline passes. Dropping it first
/// removes its scheduler entry immediately. Race it via `select` — see
/// `bounded` for the common one-shot shape.
pub(crate) struct Watchdog {
    receiver: oneshot::Receiver<()>,
    slot: Slot,
}

impl std::future::Future for Watchdog {
    type Output = std::result::Result<(), oneshot::error::RecvError>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        std::pin::Pin::new(&mut self.receiver).poll(cx)
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        // Close first: dropping the removed sender then cannot invoke this
        // waiter's own Waker, so cancellation runs no caller wake at all.
        self.receiver.close();
        if let Slot::Armed { scheduler, key } = &self.slot {
            scheduler.disarm(key);
        }
    }
}

/// Arm a deadline `timeout` from now on the process-wide scheduler.
pub(crate) fn watchdog(timeout: Duration) -> std::result::Result<Watchdog, DeadlineUnavailable> {
    SCHEDULER.arm(timeout)
}

/// Bounded wait for assertions that must NOT resolve in time: the
/// deadline is the [`watchdog`] arm raced against the future via
/// `select`. Returns `None` on deadline (the expected arm for "must
/// not leak" rows). Unit-tier arm: the contract tiers carry their own
/// copy in `tests/common` (they are separate crates).
#[cfg(test)]
pub(crate) async fn bounded<T>(
    millis: u64,
    fut: impl std::future::Future<Output = T>,
) -> Option<T> {
    let rx = watchdog(std::time::Duration::from_millis(millis)).expect("test deadline arms");
    tokio::pin!(fut);
    tokio::pin!(rx);
    tokio::select! {
        out = &mut fut => Some(out),
        _ = &mut rx => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Barrier};

    /// Isolated scheduler per test: the process-wide one is shared by every
    /// concurrently running unit test, so exact entry counts need their own.
    fn scheduler() -> &'static DeadlineScheduler {
        Box::leak(Box::new(DeadlineScheduler::new()))
    }

    fn fired(watchdog: &mut Watchdog) -> bool {
        matches!(watchdog.receiver.try_recv(), Ok(()))
    }

    /// Due entries leave the map under the lock and fire just after it is
    /// released, so observe the firing itself rather than an empty map.
    fn wait_fired(watchdog: &mut Watchdog, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while !fired(watchdog) {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        true
    }

    fn wait_until(timeout: Duration, condition: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        while !condition() {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        true
    }

    #[test]
    fn deadline_fires_no_earlier_than_its_monotonic_deadline() {
        let scheduler = scheduler();
        let armed_at = Instant::now();
        let mut watchdog = scheduler.arm(Duration::from_millis(60)).unwrap();
        // Load-independent: read the firing before the clock, so an early
        // fire is caught even if this thread stalls past the deadline.
        let fired_early = fired(&mut watchdog);
        assert!(!fired_early || armed_at.elapsed() >= Duration::from_millis(60));
        assert!(fired_early || wait_fired(&mut watchdog, Duration::from_secs(5)));
        assert!(armed_at.elapsed() >= Duration::from_millis(60));
        assert_eq!(scheduler.armed_len(), 0);
    }

    #[test]
    fn unrepresentable_deadline_never_elapses_and_needs_no_scheduler() {
        let scheduler = scheduler();
        scheduler.refuse_spawn.store(true, Ordering::SeqCst);
        let mut forever = scheduler.arm(Duration::MAX).unwrap();
        assert_eq!(scheduler.armed_len(), 0);
        assert!(!scheduler.schedule.lock().worker_running);
        std::thread::sleep(Duration::from_millis(20));
        assert!(matches!(
            forever.receiver.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn zero_timeout_fires_immediately_without_a_scheduler() {
        let scheduler = scheduler();
        scheduler.refuse_spawn.store(true, Ordering::SeqCst);
        let mut due = scheduler.arm(Duration::ZERO).unwrap();
        assert!(fired(&mut due));
        assert_eq!(scheduler.armed_len(), 0);
    }

    #[test]
    fn refused_scheduler_thread_is_reported_and_retried() {
        let scheduler = scheduler();
        scheduler.refuse_spawn.store(true, Ordering::SeqCst);
        assert!(matches!(
            scheduler.arm(Duration::from_secs(60)),
            Err(DeadlineUnavailable)
        ));
        assert_eq!(scheduler.armed_len(), 0, "a refused arm inserts nothing");
        assert!(!scheduler.schedule.lock().worker_running);

        scheduler.refuse_spawn.store(false, Ordering::SeqCst);
        let mut retried = scheduler.arm(Duration::from_millis(20)).unwrap();
        assert!(wait_fired(&mut retried, Duration::from_secs(5)));
        assert_eq!(scheduler.armed_len(), 0);
    }

    #[test]
    fn cancelled_deadlines_leave_no_entries() {
        let scheduler = scheduler();
        let watchdogs = (0..1_000)
            .map(|i| scheduler.arm(Duration::from_secs(600 + i)).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(scheduler.armed_len(), 1_000);
        drop(watchdogs);
        assert_eq!(scheduler.armed_len(), 0);
    }

    #[test]
    fn earlier_insertion_wakes_a_worker_parked_on_a_later_deadline() {
        let scheduler = scheduler();
        let _late = scheduler.arm(Duration::from_secs(600)).unwrap();
        // Let the worker park until the late deadline before inserting.
        std::thread::sleep(Duration::from_millis(20));
        let armed_at = Instant::now();
        let mut early = scheduler.arm(Duration::from_millis(30)).unwrap();
        assert!(wait_fired(&mut early, Duration::from_secs(5)));
        assert!(armed_at.elapsed() >= Duration::from_millis(30));
        assert_eq!(scheduler.armed_len(), 1, "only the late deadline remains");
    }

    #[test]
    fn cancelling_the_earliest_deadline_keeps_later_ones_firing() {
        let scheduler = scheduler();
        let earliest = scheduler.arm(Duration::from_millis(40)).unwrap();
        let mut later = scheduler.arm(Duration::from_millis(80)).unwrap();
        drop(earliest);
        assert!(wait_fired(&mut later, Duration::from_secs(5)));
        assert_eq!(scheduler.armed_len(), 0);
    }

    /// Concurrent arms and cancellations across threads: every surviving short
    /// deadline fires, every cancelled one is removed, nothing is left behind.
    #[test]
    fn concurrent_insertion_and_cancellation_do_not_lose_wakes_or_entries() {
        const THREADS: usize = 8;
        const ROUNDS: usize = 200;
        let scheduler = scheduler();
        let _parked = scheduler.arm(Duration::from_secs(600)).unwrap();
        let start = Arc::new(Barrier::new(THREADS));
        let workers = (0..THREADS)
            .map(|thread| {
                let start = start.clone();
                std::thread::spawn(move || {
                    start.wait();
                    let mut kept = Vec::new();
                    for round in 0..ROUNDS {
                        let millis = 1 + ((thread * ROUNDS + round) % 25) as u64;
                        let watchdog = scheduler.arm(Duration::from_millis(millis)).unwrap();
                        if round % 2 == 0 {
                            drop(watchdog);
                        } else {
                            kept.push(watchdog);
                        }
                    }
                    kept
                })
            })
            .collect::<Vec<_>>();
        let mut kept = workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        assert!(
            wait_until(Duration::from_secs(10), || scheduler.armed_len() == 1),
            "short deadlines must all fire; {} still armed",
            scheduler.armed_len() - 1
        );
        assert_eq!(kept.len(), THREADS * ROUNDS / 2);
        assert!(
            kept.iter_mut()
                .all(|watchdog| wait_fired(watchdog, Duration::from_secs(5)))
        );
    }

    struct PanickingWake;
    impl std::task::Wake for PanickingWake {
        fn wake(self: Arc<Self>) {
            panic!("caller waker panicked");
        }
    }

    /// Arm `timeout` with a Waker that panics on delivery. The Waker is
    /// registered before the entry is scheduled, so no stall can let the
    /// deadline fire first.
    fn arm_with_panicking_waker(
        scheduler: &'static DeadlineScheduler,
        timeout: Duration,
    ) -> Watchdog {
        let (sender, mut receiver) = oneshot::channel();
        let waker = std::task::Waker::from(Arc::new(PanickingWake));
        let mut cx = std::task::Context::from_waker(&waker);
        assert!(std::pin::Pin::new(&mut receiver).poll(&mut cx).is_pending());
        let key = scheduler.insert(Instant::now() + timeout, sender).unwrap();
        Watchdog {
            receiver,
            slot: Slot::Armed { scheduler, key },
        }
    }

    #[test]
    fn a_panicking_wake_does_not_stop_delivery_to_other_arms() {
        let scheduler = scheduler();
        let mut poisoned = arm_with_panicking_waker(scheduler, Duration::from_millis(20));
        // Due in the same batch as the poisoned wake, and in a later batch.
        let mut same_batch = scheduler.arm(Duration::from_millis(20)).unwrap();
        let mut later = scheduler.arm(Duration::from_millis(80)).unwrap();
        assert!(wait_fired(&mut same_batch, Duration::from_secs(5)));
        assert!(wait_fired(&mut later, Duration::from_secs(5)));
        assert!(
            fired(&mut poisoned),
            "the value is delivered even though its wake panicked"
        );
        assert!(scheduler.schedule.lock().worker_running);
    }

    #[test]
    fn cancelling_an_arm_runs_no_wake() {
        let scheduler = scheduler();
        let cancelled = arm_with_panicking_waker(scheduler, Duration::from_secs(600));
        // Would panic here if dropping the removed sender woke the receiver.
        drop(cancelled);
        assert_eq!(scheduler.armed_len(), 0);
    }

    #[test]
    fn an_unexpected_serving_panic_resumes_without_a_new_arm() {
        let scheduler = scheduler();
        let mut armed = scheduler.arm(Duration::from_millis(60)).unwrap();
        scheduler.panic_serving_once.store(true, Ordering::SeqCst);
        // Wake the worker without arming anything: it panics on this pass.
        scheduler.changed.notify_one();
        assert!(wait_fired(&mut armed, Duration::from_secs(5)));
        assert!(!scheduler.panic_serving_once.load(Ordering::SeqCst));
        assert!(scheduler.schedule.lock().worker_running);
    }

    #[test]
    fn a_lost_worker_is_replaced_for_already_armed_deadlines() {
        let scheduler = scheduler();
        let mut armed = scheduler.arm(Duration::from_millis(60)).unwrap();
        scheduler.exit_worker_once.store(true, Ordering::SeqCst);
        scheduler.changed.notify_one();
        assert!(wait_fired(&mut armed, Duration::from_secs(5)));
        assert!(!scheduler.exit_worker_once.load(Ordering::SeqCst));
        assert!(scheduler.schedule.lock().worker_running);
    }

    #[test]
    fn a_refused_replacement_keeps_deadlines_armed_until_the_next_arm() {
        let scheduler = scheduler();
        let mut armed = scheduler.arm(Duration::from_millis(40)).unwrap();
        scheduler.refuse_spawn.store(true, Ordering::SeqCst);
        scheduler.exit_worker_once.store(true, Ordering::SeqCst);
        scheduler.changed.notify_one();
        assert!(wait_until(Duration::from_secs(5), || {
            !scheduler.schedule.lock().worker_running
        }));
        std::thread::sleep(Duration::from_millis(80));
        // Past its deadline but never dropped: no premature or false Elapsed.
        assert!(matches!(
            armed.receiver.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert_eq!(scheduler.armed_len(), 1);

        scheduler.refuse_spawn.store(false, Ordering::SeqCst);
        let _next = scheduler.arm(Duration::from_secs(600)).unwrap();
        assert!(wait_fired(&mut armed, Duration::from_secs(5)));
    }
}
