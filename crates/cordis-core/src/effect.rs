//! Effects — generation-owned cleanup obligations with exact at-most-once
//! claims.
//!
//! Every side effect (listener registration, service publication, a
//! fiber-bound task, a plain cleanup) registers exactly one cleanup
//! obligation into the **current open generation** of the selected fiber.
//! The generation owns its obligations: when the generation closes —
//! unload, restart/update replacement, failed-apply rollback, or disposal —
//! the framework claims every remaining obligation and runs it in strict
//! sequential reverse-commit order, attempting all of them even when some
//! fail.
//!
//! Registration returns a move-only [`EffectRegistration`]: the exact
//! capability for claiming that one occurrence early. Manual control and
//! the generation drain arbitrate one exact claim — whichever removes the
//! occurrence first owns it, and the loser observes `false`. A winning
//! [`EffectRegistration::dispose`] transfers completion to framework
//! ownership: the cleanup reaches its end independently of caller polling.
//! A cleanup that returns an error or panics permanently consumes the
//! occurrence; the failure normalizes once into [`EffectFailure`].
//!
//! Cleanups are `FnOnce + Send + 'static` (never reusable callbacks) whose
//! result adapts through the sealed [`CleanupResult`]: `()` for infallible
//! cleanup, `Result<(), E>` for a fallible one. Async cleanups return Send
//! futures. Registering through a context whose fiber generation is closed
//! (stable Pending or Failed, draining, or disposed) fails fast with
//! [`EffectRegistrationError::InactiveContext`] and runs nothing.

use crate::context::Context;
use crate::fiber::Fiber;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Weak};

/// A boxed, owned, sendable future — the erased currency of listener and
/// tail signatures.
///
/// Crate-private boxed-future representation used only across internal owner seams.
/// Public semantic operations expose their concrete `impl Future` forms instead.
pub(crate) type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// One stored cleanup obligation: a one-shot closure whose execution
/// resolves to its already-normalized failure, if any. Async cleanup starts
/// on Cordis's completion runtime so runtime-bound futures are never first
/// polled on an executor that may later disappear; synchronous bookkeeping
/// stays on the owning lifecycle executor to preserve commit ordering.
pub(crate) struct Cleanup {
    run: Box<dyn FnOnce() -> BoxFuture<Result<(), EffectFailure>> + Send>,
    async_cleanup: bool,
}

impl Cleanup {
    pub(crate) fn async_cleanup(&self) -> bool {
        self.async_cleanup
    }
}

/// Adapt an infallible synchronous cleanup (resource bookkeeping: store
/// withdrawal, hook removal) into the journal's shape.
pub(crate) fn sync_cleanup<F>(f: F) -> Cleanup
where
    F: FnOnce() + Send + 'static,
{
    Cleanup {
        run: Box::new(move || {
            Box::pin(async move {
                f();
                Ok(())
            })
        }),
        async_cleanup: false,
    }
}

/// Adapt an infallible async cleanup into the journal's shape. The call
/// happens on the first poll of the returned future, so a panic at call
/// time is inside the containment boundary, not at the adapter.
pub(crate) fn fut_cleanup<F>(f: F) -> Cleanup
where
    F: FnOnce() -> BoxFuture<()> + Send + 'static,
{
    Cleanup {
        run: Box::new(move || {
            Box::pin(async move {
                f().await;
                Ok(())
            })
        }),
        async_cleanup: true,
    }
}

/// Run one claimed cleanup to completion: invoke, await, contain.
///
/// The closure call itself is inside the caught future — a synchronous
/// cleanup panics at call time (before any future exists to await) and an
/// async one panics on poll, so both phases must be within the boundary.
/// A returned error arrives already normalized by [`CleanupResult`]; a
/// panic is caught here and normalized into the same [`EffectFailure`].
/// Either way the occurrence is consumed: nothing restores it.
pub(crate) async fn execute_cleanup(cleanup: Cleanup) -> Option<EffectFailure> {
    let run = cleanup.run;
    match crate::contained::catch_contained(async move { run().await }).await {
        Ok(Ok(())) => None,
        Ok(Err(failure)) => Some(failure),
        Err(payload) => Some(EffectFailure::panicked(crate::contained::payload_text(
            &payload,
        ))),
    }
}

/// Report one claimed cleanup's failure through the containment
/// boundary's single routing (`cordis:`-prefixed warn record, stderr
/// fallback). Used when nobody is left to receive the failure: the
/// generation drain, and a winning manual dispose whose caller abandoned
/// the wait. A failure is delivered exactly once — returned to a waiting
/// caller, or reported here; never both.
pub(crate) fn report_cleanup_failure(
    logger: Option<&crate::logger::Logger>,
    failure: &EffectFailure,
) {
    crate::contained::report_text(logger, format!("cordis: effect cleanup {failure}"));
}

/// Opaque key of one journal occurrence, handed to the matching
/// [`EffectRegistration`] claim capability. Crate-internal: nothing public
/// constructs or consumes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct DisposableToken(u64);

/// The per-generation cleanup journal: a keyed list of cleanup obligations
/// preserving insertion (commit) order. `tokens()` returns keys in forward
/// order and the owning fiber's drain runs cleanups LIFO by reversing it;
/// `remove` is the exact one-time claim both manual control and the drain
/// compete on. The journal holds no labels and answers no introspection —
/// presence of a resource is observed through that resource's own semantic
/// interface, never through cleanup bookkeeping.
#[derive(Default)]
pub(crate) struct DisposableList {
    sn: u64,
    map: BTreeMap<u64, Cleanup>,
}

impl DisposableList {
    /// An empty journal.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Commit one cleanup obligation; returns its exact claim token.
    pub(crate) fn push(&mut self, cleanup: Cleanup) -> DisposableToken {
        self.sn += 1;
        let sn = self.sn;
        self.map.insert(sn, cleanup);
        DisposableToken(sn)
    }

    /// Claim a previously pushed occurrence, returning its cleanup if it
    /// was still owned by the generation — the remove-returns-ownership
    /// two-phase shape (ADR 0010): the caller runs or drops the cleanup
    /// *after* releasing the lock, so user-controlled destruction never
    /// executes inside the critical section.
    pub(crate) fn remove(&mut self, token: DisposableToken) -> Option<Cleanup> {
        self.map.remove(&token.0)
    }

    /// Snapshot of every still-owned token, in commit order — the drain's
    /// worklist (it reverses this for LIFO).
    pub(crate) fn tokens(&self) -> Vec<DisposableToken> {
        self.map.keys().map(|k| DisposableToken(*k)).collect()
    }
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for () {}
    impl<E: std::error::Error> Sealed for Result<(), E> {}
}

/// What an effect cleanup may return. Sealed: exactly `()` (infallible)
/// and `Result<(), E>` for `E: Error` (fallible) are adapted — the last
/// boundary that still knows the error's type, so a failure normalizes
/// exactly once into the opaque [`EffectFailure`] and the original error
/// object, `Any` access, and downcasts never escape.
pub trait CleanupResult: sealed::Sealed {
    #[doc(hidden)]
    fn into_outcome(self) -> std::result::Result<(), EffectFailure>;
}

impl CleanupResult for () {
    fn into_outcome(self) -> std::result::Result<(), EffectFailure> {
        Ok(())
    }
}

impl<E: std::error::Error> CleanupResult for Result<(), E> {
    fn into_outcome(self) -> std::result::Result<(), EffectFailure> {
        self.map_err(|e| EffectFailure::returned(e.to_string()))
    }
}

/// The semantic kind of an [`EffectFailure`]: the cleanup returned an
/// error, or it panicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EffectFailureKind {
    /// The cleanup returned `Err(_)` from its `Result<(), E>` form.
    ReturnedError,
    /// The cleanup panicked (at call time or while its future was
    /// polled); the panic was contained at the cleanup boundary.
    Panic,
}

/// One cleanup's normalized failure: its semantic [`EffectFailureKind`]
/// plus owned diagnostic text. Opaque on purpose — no public constructor,
/// no original error object, no downcast: the failure is reported and
/// matched by kind, never re-thrown or inspected by type.
pub struct EffectFailure {
    kind: EffectFailureKind,
    diagnostic: String,
}

impl EffectFailure {
    pub(crate) fn returned(diagnostic: String) -> Self {
        Self {
            kind: EffectFailureKind::ReturnedError,
            diagnostic,
        }
    }

    pub(crate) fn panicked(payload: String) -> Self {
        Self {
            kind: EffectFailureKind::Panic,
            diagnostic: payload,
        }
    }

    /// Whether the cleanup returned an error or panicked.
    pub fn kind(&self) -> EffectFailureKind {
        self.kind
    }

    /// The owned diagnostic text: the returned error's `Display`, or the
    /// rendered panic payload.
    pub fn diagnostic(&self) -> &str {
        &self.diagnostic
    }
}

impl std::fmt::Debug for EffectFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EffectFailure")
            .field("kind", &self.kind)
            .field("diagnostic", &self.diagnostic)
            .finish()
    }
}

impl std::fmt::Display for EffectFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            EffectFailureKind::ReturnedError => {
                write!(f, "returned an error: {}", self.diagnostic)
            }
            EffectFailureKind::Panic => write!(f, "panicked: {}", self.diagnostic),
        }
    }
}

impl std::error::Error for EffectFailure {}

/// Why an effect registration was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum EffectRegistrationError {
    /// The selected fiber's generation is not open: it is stably Pending
    /// or Failed, draining, or disposed. Nothing was registered and the
    /// cleanup stays with the caller for rollback.
    #[error("the context's fiber generation is closed to new cleanup")]
    InactiveContext,
}

/// Why a task registration was refused.
///
/// Both refusal arms are pre-commit: nothing was registered into the
/// generation and no task was started — the would-be task future drops
/// with the caller, outside every lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TaskRegistrationError {
    /// The selected fiber's generation is not open: it is stably Pending
    /// or Failed, draining, or disposed.
    #[error("the context's fiber generation is closed to new tasks")]
    InactiveContext,
    /// No async runtime is current on this thread to drive the task.
    #[error("no async runtime is current")]
    ExecutorUnavailable,
}

/// The exact, move-only claim capability for one registered cleanup
/// occurrence.
///
/// Both operations consume the registration, so the two manual operations
/// cannot race each other; each races only the generation drain:
///
/// - [`disarm`](Self::disarm) releases the occurrence *without* running
///   the cleanup — for resources that finished naturally;
/// - [`dispose`](Self::dispose) runs the cleanup now, under framework
///   ownership: once the claim is won, completion no longer depends on
///   the caller polling the returned future.
///
/// Whichever side — manual control or the generation drain — claims the
/// occurrence first owns it; the consumed loser reports `false` and runs
/// nothing again. Dropping the registration is inert: the occurrence stays
/// generation-owned and the drain runs it when the generation closes.
pub struct EffectRegistration {
    token: DisposableToken,
    owner: Weak<Fiber>,
}

impl EffectRegistration {
    /// Release this cleanup occurrence **without running it**. `true` when
    /// the claim was won here; `false` when the generation drain (or an
    /// earlier claim) already owns or consumed the occurrence.
    pub fn disarm(self) -> bool {
        let Some(fiber) = self.owner.upgrade() else {
            return false;
        };
        // the claimed cleanup is dropped here, in the caller's frame —
        // `remove_disposable` has already released the journal lock, so
        // captures whose Drop re-enters the fiber cannot deadlock
        // (ADR 0010 two-phase)
        fiber.remove_disposable(self.token).is_some()
    }

    /// Run this cleanup now and keep the generation drain from running it
    /// again. `Ok(true)` when this call claimed the occurrence and the
    /// cleanup completed; `Ok(false)` when the drain already claimed it
    /// (nothing runs twice); `Err(EffectFailure)` when this call claimed
    /// the occurrence and the cleanup returned an error or panicked — the
    /// occurrence is permanently consumed either way.
    ///
    /// Once the claim wins, the cleanup's completion is framework-owned:
    /// dropping this future after the claim abandons only the wait, and a
    /// failure nobody is left to receive is reported through the fiber's
    /// diagnostics instead. A live settle attribution at the claim site is
    /// carried into the detached cleanup task, so an apply/disposer awaiting
    /// this manual cleanup cannot lose same-Fiber recursion refusal across the
    /// task boundary. An ordinary external caller carries no such attribution.
    /// Dropping the future *before* its first poll changes nothing — the
    /// occurrence stays generation-owned.
    pub async fn dispose(self) -> std::result::Result<bool, EffectFailure> {
        let Some(fiber) = self.owner.upgrade() else {
            return Ok(false);
        };
        let Some(cleanup) = fiber.remove_disposable(self.token) else {
            return Ok(false);
        };
        // The claim is won: from here completion is framework-owned. Async
        // cleanup starts on Cordis's completion runtime so runtime-bound work
        // created by the callback never migrates between Tokio drivers. Sync
        // cleanup preserves lifecycle-executor ordering and may transfer only
        // if executor shutdown drops its pending Cordis wrapper. The outcome
        // travels back through one-shot; caller cancellation abandons only the
        // wait. Runtime-bound resources captured earlier from an external
        // runtime remain that runtime's responsibility. Capture any live
        // settle dependency before crossing the task boundary; an ordinary
        // external manual-dispose call captures the empty attribution.
        let async_cleanup = cleanup.async_cleanup();
        let attribution = crate::fiber::capture_settle_attribution();
        let logger = fiber.fiber_ctx().map(|ctx| ctx.logger());
        let (tx, rx) = tokio::sync::oneshot::channel();
        let work = crate::fiber::with_settle_attribution(attribution, async move {
            let outcome = execute_cleanup(cleanup).await;
            if let Err(outcome) = tx.send(outcome)
                && let Some(failure) = outcome
            {
                // the caller abandoned the wait — the failure still gets
                // its exactly-one diagnostic report
                report_cleanup_failure(logger.as_ref(), &failure);
            }
        });
        if async_cleanup {
            detach_cleanup(work);
        } else {
            detach(work);
        }
        match rx.await {
            Ok(None) => Ok(true),
            Ok(Some(failure)) => Err(failure),
            // Only unexpected framework-task termination can close without
            // publishing; the exact claim remains consumed either way.
            Err(_closed) => Ok(true),
        }
    }
}

impl Context {
    /// Register an async cleanup as one obligation of this context's
    /// current fiber generation — the path for cleanups with something to
    /// await. The closure runs at most once: claimed by the generation
    /// drain (LIFO) or early by the returned [`EffectRegistration`]. A
    /// cleanup with nothing to await belongs on
    /// [`Context::effect_sync`] instead. Framework-owned execution starts on
    /// Cordis's process-wide completion runtime, so Tokio time/IO primitives
    /// created by the cleanup bind there and are independent of the caller's
    /// runtime lifetime. Runtime-bound resources captured before cleanup
    /// execution remain tied to the external runtime that created them.
    ///
    /// Fails with [`EffectRegistrationError::InactiveContext`] when the
    /// generation is closed (stable Pending or Failed, draining, or
    /// disposed); nothing is registered then and the cleanup drops with
    /// the caller, outside every lock.
    pub fn effect<F, Fut, R>(
        &self,
        cleanup: F,
    ) -> std::result::Result<EffectRegistration, EffectRegistrationError>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = R> + Send,
        R: CleanupResult,
    {
        self.register_cleanup(Cleanup {
            run: Box::new(move || Box::pin(async move { cleanup().await.into_outcome() })),
            async_cleanup: true,
        })
    }

    /// Register a **synchronous** cleanup as one obligation of this
    /// context's current fiber generation — the spelling for cleanups
    /// with nothing to await, so no call site needs a `Box::pin(async {})`
    /// stub. Claim and failure semantics match [`Context::effect`], while
    /// synchronous execution keeps lifecycle-executor ordering. The callback
    /// must remain short and must not block indefinitely: off-runtime or
    /// shutdown-resilient completion may run it on Cordis's shared completion
    /// runtime, where blocking a worker can delay unrelated framework cleanup.
    /// For blocking work, register an async [`Context::effect`] and offload the
    /// blocking section with `tokio::task::spawn_blocking`.
    pub fn effect_sync<F, R>(
        &self,
        cleanup: F,
    ) -> std::result::Result<EffectRegistration, EffectRegistrationError>
    where
        F: FnOnce() -> R + Send + 'static,
        R: CleanupResult,
    {
        self.register_cleanup(Cleanup {
            run: Box::new(move || Box::pin(async move { cleanup().into_outcome() })),
            async_cleanup: false,
        })
    }

    /// The one registration path both spellings share: the closure is
    /// already wrapped into the journal's normalized shape (construction
    /// of the closure happened at the call site, conversion of its result
    /// happens at execution — neither under the journal lock).
    fn register_cleanup(
        &self,
        cleanup: Cleanup,
    ) -> std::result::Result<EffectRegistration, EffectRegistrationError> {
        // a plain effect publishes nothing but its cleanup obligation, so
        // the gated seam can only refuse on admission — map it onto the
        // operation's one variant
        let token = crate::gated::push_gated(self.fiber(), cleanup, &mut crate::gated::NoPublish)
            .map_err(|_| EffectRegistrationError::InactiveContext)?;
        Ok(EffectRegistration {
            token,
            owner: Arc::downgrade(self.fiber()),
        })
    }
}

/// Process-wide executor for work that must outlive the caller's Tokio runtime.
/// It is initialized lazily and has fixed worker-thread cost for Runtime lifetime.
fn completion_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("cordis-completion")
            .enable_all()
            .build()
            .expect("Cordis completion runtime construction must succeed")
    })
}

/// Wrapper used only for Cordis-owned lifecycle futures. These futures are
/// runtime-agnostic after user async cleanup has been split onto
/// [`detach_cleanup`]. A normal `Pending` authorizes transfer if the origin
/// executor drops the task during shutdown; a poll unwind never does.
struct DetachedWork<F>
where
    F: Future<Output = ()> + Send + 'static,
{
    work: Option<Pin<Box<F>>>,
    transfer_on_drop: bool,
}

impl<F> Future for DetachedWork<F>
where
    F: Future<Output = ()> + Send + 'static,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        let this = self.get_mut();
        this.transfer_on_drop = false;
        let work = this
            .work
            .as_mut()
            .expect("live detached work owns its future");
        match work.as_mut().poll(cx) {
            std::task::Poll::Pending => {
                this.transfer_on_drop = true;
                std::task::Poll::Pending
            }
            std::task::Poll::Ready(()) => {
                this.work.take();
                std::task::Poll::Ready(())
            }
        }
    }
}

impl<F> Drop for DetachedWork<F>
where
    F: Future<Output = ()> + Send + 'static,
{
    fn drop(&mut self) {
        if self.transfer_on_drop
            && let Some(work) = self.work.take()
        {
            let _join = completion_runtime().spawn(work);
        }
    }
}

/// Detach Cordis-owned runtime-agnostic lifecycle work. Normal execution stays
/// on the current Tokio runtime to preserve scheduling semantics; if that
/// runtime shuts down while the task is pending, the same pinned future moves
/// to the shared completion runtime. Off-runtime callers start there directly.
pub(crate) fn detach(work: impl Future<Output = ()> + Send + 'static) {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            let _join = handle.spawn(DetachedWork {
                work: Some(Box::pin(work)),
                transfer_on_drop: true,
            });
        }
        Err(_) => {
            let _join = completion_runtime().spawn(work);
        }
    }
}

/// Start runtime-bound framework work on the shared completion runtime, so
/// futures created during cleanup, Plugin apply, or dependent convergence do
/// not migrate between Tokio drivers when the caller's runtime shuts down.
pub(crate) fn spawn_completion(
    work: impl Future<Output = ()> + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    completion_runtime().spawn(work)
}

/// Detach an async cleanup on the completion runtime from its first poll.
pub(crate) fn detach_cleanup(work: impl Future<Output = ()> + Send + 'static) {
    let _join = spawn_completion(work);
}

#[cfg(test)]
mod tests {
    use super::{DetachedWork, DisposableList, EffectFailureKind, sync_cleanup};
    use std::future::Future;
    use std::pin::Pin;

    struct PollPanic(std::sync::mpsc::Sender<()>);

    impl Future for PollPanic {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
            self.0.send(()).unwrap();
            panic!("detached poll probe");
        }
    }

    #[test]
    fn detached_poll_panic_is_not_retried_by_fallback() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut detached = Box::pin(DetachedWork {
            work: Some(Box::pin(PollPanic(tx))),
            transfer_on_drop: true,
        });
        let waker = std::task::Waker::noop();
        let mut cx = std::task::Context::from_waker(waker);

        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            detached.as_mut().poll(&mut cx)
        }));
        assert!(outcome.is_err());
        rx.recv_timeout(std::time::Duration::from_secs(1))
            .expect("probe reached its first poll");

        drop(detached);
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(100))
                .is_err(),
            "poll panic must not transfer the same future to fallback"
        );
    }

    // The claim seam the drain and manual control compete on: `remove`
    // hands the cleanup out instead of dropping it in place — the
    // lock-side half of the discipline the owner's call sites complete
    // (ADR 0010), and exactly-once by construction.
    #[test]
    fn remove_claims_the_occurrence_exactly_once() {
        let mut list = DisposableList::new();
        let token = list.push(sync_cleanup(|| {}));
        assert!(list.remove(token).is_some(), "first claim hands it out");
        assert!(
            list.remove(token).is_none(),
            "a token resolves exactly once"
        );
        assert!(list.tokens().is_empty());
    }

    // The drain's worklist is commit order; the fiber reverses it for
    // LIFO — pinned here so the journal and the drain cannot drift apart.
    #[test]
    fn tokens_snapshot_commit_order() {
        let mut list = DisposableList::new();
        let a = list.push(sync_cleanup(|| {}));
        let b = list.push(sync_cleanup(|| {}));
        assert_eq!(
            list.tokens(),
            vec![a, b],
            "forward order — the drain reverses it"
        );
        list.remove(a);
        assert_eq!(list.tokens(), vec![b]);
    }

    #[test]
    fn cleanup_result_normalizes_once() {
        use super::CleanupResult;
        assert!(().into_outcome().is_ok());
        #[derive(Debug, thiserror::Error)]
        #[error("cleanup boom")]
        struct Boom;
        let failure = Err::<(), Boom>(Boom).into_outcome().unwrap_err();
        assert_eq!(failure.kind(), EffectFailureKind::ReturnedError);
        assert_eq!(failure.diagnostic(), "cleanup boom");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cleanup_can_reenter_same_generation_registration() {
        use crate::Context;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::Duration;

        let ctx = Context::new();
        let reentered = Arc::new(AtomicBool::new(false));
        let cleanup_ctx = ctx.clone();
        let cleanup_reentered = reentered.clone();
        let registration = ctx
            .effect_sync(move || {
                cleanup_ctx
                    .effect_sync(|| {})
                    .expect("cleanup may reenter the same generation journal");
                cleanup_reentered.store(true, Ordering::SeqCst);
            })
            .unwrap();

        let dispose = registration.dispose();
        tokio::pin!(dispose);
        let watchdog = crate::deadline::watchdog(Duration::from_secs(2));
        tokio::pin!(watchdog);
        tokio::select! {
            result = &mut dispose => assert_eq!(result.unwrap(), true),
            _ = &mut watchdog => panic!("cleanup reentry deadlocked generation bookkeeping"),
        }
        assert!(reentered.load(Ordering::SeqCst));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cleanup_error_conversion_can_reenter_generation_bookkeeping() {
        use crate::Context;
        use std::fmt;
        use std::time::Duration;

        struct ReentrantError(Context);

        impl fmt::Debug for ReentrantError {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("ReentrantError")
            }
        }

        impl fmt::Display for ReentrantError {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0
                    .effect_sync(|| {})
                    .expect("cleanup error conversion may reenter the generation journal");
                f.write_str("issue55 reentrant cleanup error")
            }
        }

        impl std::error::Error for ReentrantError {}

        let ctx = Context::new();
        let conversion_ctx = ctx.clone();
        let registration = ctx
            .effect_sync(move || -> Result<(), ReentrantError> {
                Err(ReentrantError(conversion_ctx))
            })
            .unwrap();

        let dispose = registration.dispose();
        tokio::pin!(dispose);
        let watchdog = crate::deadline::watchdog(Duration::from_secs(2));
        tokio::pin!(watchdog);
        let failure = tokio::select! {
            result = &mut dispose => result.expect_err("cleanup returns the probe error"),
            _ = &mut watchdog => panic!("cleanup error conversion deadlocked generation bookkeeping"),
        };
        assert_eq!(failure.kind(), EffectFailureKind::ReturnedError);
        assert_eq!(failure.diagnostic(), "issue55 reentrant cleanup error");
    }
}
