//! Runtime-agnostic future execution.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Wake, Waker};

/// A boxed, sendable future used by public Cordis callbacks.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

#[derive(Default)]
struct Parker {
    notified: AtomicBool,
    lock: Mutex<()>,
    ready: Condvar,
}

impl Wake for Parker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        // Hold the mutex so set-and-notify is serialized against the waiter's
        // check-and-wait in block_on. Without it a wake landing between the
        // `notified` check and `wait` is lost and block_on never returns.
        let _guard = lock(&self.lock);
        self.notified.store(true, Ordering::Release);
        self.ready.notify_one();
    }
}

/// Drive a future to completion without choosing an async runtime.
///
/// Plugin and disposer futures can still use any executor-independent future.
/// Runtime-specific resources (for example Tokio timers) should be entered by
/// the application before invoking Cordis.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let parker = Arc::new(Parker::default());
    let waker = Waker::from(parker.clone());
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);

    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }

        let mut guard = parker
            .lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        while !parker.notified.swap(false, Ordering::AcqRel) {
            guard = parker
                .ready
                .wait(guard)
                .unwrap_or_else(|error| error.into_inner());
        }
    }
}

pub(crate) async fn join_all<T>(futures: Vec<BoxFuture<T>>) -> Vec<T> {
    struct JoinAll<T> {
        futures: Vec<Option<BoxFuture<T>>>,
        outputs: Vec<Option<T>>,
        remaining: usize,
    }

    impl<T> Unpin for JoinAll<T> {}

    impl<T> Future for JoinAll<T> {
        type Output = Vec<T>;

        fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
            let this = self.as_mut().get_mut();
            for index in 0..this.futures.len() {
                let ready = match this.futures[index].as_mut() {
                    Some(future) => match future.as_mut().poll(context) {
                        Poll::Ready(output) => Some(output),
                        Poll::Pending => None,
                    },
                    None => None,
                };
                if let Some(output) = ready {
                    this.futures[index] = None;
                    this.outputs[index] = Some(output);
                    this.remaining -= 1;
                }
            }
            if this.remaining == 0 {
                Poll::Ready(
                    this.outputs
                        .iter_mut()
                        .map(|output| output.take().expect("completed future"))
                        .collect(),
                )
            } else {
                Poll::Pending
            }
        }
    }

    let count = futures.len();
    JoinAll {
        futures: futures.into_iter().map(Some).collect(),
        outputs: (0..count).map(|_| None).collect(),
        remaining: count,
    }
    .await
}

/// Recover a mutex guard even when another callback panicked while holding it.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    /// A wake landing between the waiter's `notified` check and its block on
    /// the condvar must not be lost. Holding the mutex across the wake widens
    /// that window deterministically: pre-fix `wake_by_ref` completes without
    /// the mutex and its notification evaporates, so the wait below times out.
    #[test]
    fn wake_between_check_and_wait_is_not_lost() {
        let parker = Arc::new(Parker::default());
        let waker = Waker::from(parker.clone());

        // Simulate the waiter in block_on: holding the mutex, it has just
        // observed `notified == false` and is about to wait.
        let guard = lock(&parker.lock);
        assert!(!parker.notified.swap(false, Ordering::AcqRel));

        // Fire the wake from another thread, inside that window.
        let waker_thread = thread::spawn(move || waker.wake_by_ref());
        thread::sleep(Duration::from_millis(100));

        let (_guard, timeout) = parker
            .ready
            .wait_timeout(guard, Duration::from_secs(2))
            .unwrap_or_else(|error| error.into_inner());
        assert!(!timeout.timed_out(), "lost wakeup: block_on would hang");
        waker_thread.join().unwrap();
    }
}
