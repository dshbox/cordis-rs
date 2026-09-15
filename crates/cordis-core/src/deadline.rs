//! The std-clock watchdog idiom, written once. Core carries no tokio
//! time feature (ADR 0002 — timer discipline lives in cordis-timer), so
//! a deadline is a std thread + oneshot raced against the wait via
//! `select`. Before this module the shape was hand-copied at every
//! deadline site; both arms below state the rationale so no site
//! re-derives it.

/// Raw watchdog arm: resolves after `timeout` on its own std thread.
/// If its receiver is dropped first, the worker notices within one bounded
/// polling slice and exits instead of sleeping out the full timeout.
/// Race it via `select` — see `bounded` for the common one-shot shape.
#[cfg(test)]
use std::future::Future;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

const WATCHDOG_CANCEL_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

#[cfg(test)]
const TRACKED_WATCHDOG_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(13);
#[cfg(test)]
static LIVE_WATCHDOG_THREADS: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
pub(crate) fn tracked_watchdog_timeout() -> std::time::Duration {
    TRACKED_WATCHDOG_TIMEOUT
}

#[cfg(test)]
pub(crate) fn live_watchdog_threads() -> usize {
    LIVE_WATCHDOG_THREADS.load(Ordering::SeqCst)
}

pub(crate) fn watchdog(timeout: std::time::Duration) -> tokio::sync::oneshot::Receiver<()> {
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    #[cfg(test)]
    let tracked = timeout == TRACKED_WATCHDOG_TIMEOUT;
    #[cfg(test)]
    if tracked {
        LIVE_WATCHDOG_THREADS.fetch_add(1, Ordering::SeqCst);
    }
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        loop {
            if tx.is_closed() {
                break;
            }
            let elapsed = started.elapsed();
            if elapsed >= timeout {
                let _ = tx.send(());
                break;
            }
            let step = (timeout - elapsed).min(WATCHDOG_CANCEL_POLL_INTERVAL);
            std::thread::sleep(step);
        }
        #[cfg(test)]
        if tracked {
            LIVE_WATCHDOG_THREADS.fetch_sub(1, Ordering::SeqCst);
        }
    });
    rx
}

/// Bounded wait for assertions that must NOT resolve in time: the
/// deadline is the [`watchdog`] thread raced against the future via
/// `select`. Returns `None` on deadline (the expected arm for "must
/// not leak" rows). Unit-tier arm: the contract tiers carry their own
/// copy in `tests/common` (they are separate crates).
#[cfg(test)]
pub(crate) async fn bounded<T>(millis: u64, fut: impl Future<Output = T>) -> Option<T> {
    let rx = watchdog(std::time::Duration::from_millis(millis));
    tokio::pin!(fut);
    tokio::pin!(rx);
    tokio::select! {
        out = &mut fut => Some(out),
        _ = &mut rx => None,
    }
}
