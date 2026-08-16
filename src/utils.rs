//! Runtime-agnostic future execution and ordered disposable collections.

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
        self.notified.store(true, Ordering::Release);
        self.ready.notify_one();
    }

    fn wake_by_ref(self: &Arc<Self>) {
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

/// Ordered collection with stable numeric tokens and reverse-order clearing.
#[derive(Debug)]
pub struct DisposableList<T> {
    next: u64,
    values: Vec<(u64, T)>,
}

impl<T> Default for DisposableList<T> {
    fn default() -> Self {
        Self {
            next: 0,
            values: Vec::new(),
        }
    }
}

impl<T> DisposableList<T> {
    /// Construct an empty list.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a value and return its stable token.
    pub fn push(&mut self, value: T) -> u64 {
        self.next += 1;
        self.values.push((self.next, value));
        self.next
    }

    /// Insert before all current entries.
    pub fn unshift(&mut self, value: T) -> u64 {
        self.next += 1;
        self.values.insert(0, (self.next, value));
        self.next
    }

    /// Remove and return a value by token.
    pub fn remove(&mut self, token: u64) -> Option<T> {
        let index = self.values.iter().position(|(id, _)| *id == token)?;
        Some(self.values.remove(index).1)
    }

    /// Number of live values.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether no values are registered.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Iterate in registration order.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.values.iter().map(|(_, value)| value)
    }

    /// Drain in reverse registration order.
    pub fn clear_reverse(&mut self) -> Vec<T> {
        std::mem::take(&mut self.values)
            .into_iter()
            .rev()
            .map(|(_, value)| value)
            .collect()
    }
}

/// Recover a mutex guard even when another callback panicked while holding it.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}
