//! Complete timer operations bound to the registering Context generation.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;

use cordis_core::Context;
use cordis_core::effect::EffectRegistration;
use futures::Stream;
use futures::task::AtomicWaker;

use crate::{TimerCancelled, TimerRegistrationError};

mod private {
    pub trait Sealed {}
    impl Sealed for cordis_core::Context {}
}

/// Timer operations bound to a context's current generation.
pub trait TimerExt: private::Sealed {
    /// Construct a complete work-owning Timeout.
    ///
    /// The deadline is pinned synchronously while `work` remains lazy. The
    /// operation owns `work`; generation cleanup owns only cancellation.
    fn timeout<F: Future>(
        &self,
        delay: Duration,
        work: F,
    ) -> Result<Timeout<F>, TimerRegistrationError>;

    /// Construct one complete, deadline-pinned Sleep.
    ///
    /// Registration is synchronous and fallible. The delay may be zero. The
    /// constructor validates generation admission before the Timer environment,
    /// pins the monotonic deadline before committing generation cleanup, and
    /// returns no operation on any failure.
    fn sleep(&self, delay: Duration) -> Result<Sleep, TimerRegistrationError>;

    /// Construct a fixed-phase Interval anchored at successful construction.
    ///
    /// A zero period is rejected before Context admission and Timer environment
    /// validation. Missed ticks coalesce without shifting the construction-time
    /// phase, and generation cancellation is one explicit terminal stream item.
    fn interval(&self, period: Duration) -> Result<Interval, TimerRegistrationError>;
}

fn prepare_deadline(
    delay: Duration,
) -> Result<Pin<Box<tokio::time::Sleep>>, TimerRegistrationError> {
    if tokio::runtime::Handle::try_current().is_err() {
        return Err(TimerRegistrationError::TimerUnavailable);
    }
    let now = tokio::time::Instant::now();
    let deadline = now
        .checked_add(delay)
        .ok_or(TimerRegistrationError::DeadlineOutOfRange)?;
    std::panic::catch_unwind(|| Box::pin(tokio::time::sleep_until(deadline)))
        .map_err(|_| TimerRegistrationError::TimerUnavailable)
}

fn prepare_interval(period: Duration) -> Result<tokio::time::Interval, TimerRegistrationError> {
    if tokio::runtime::Handle::try_current().is_err() {
        return Err(TimerRegistrationError::TimerUnavailable);
    }
    let now = tokio::time::Instant::now();
    let first = now
        .checked_add(period)
        .ok_or(TimerRegistrationError::DeadlineOutOfRange)?;
    std::panic::catch_unwind(|| {
        let mut interval = tokio::time::interval_at(first, period);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval
    })
    .map_err(|_| TimerRegistrationError::TimerUnavailable)
}

fn commit_cancellation(
    ctx: &Context,
) -> Result<(Arc<GenerationCancellation>, EffectEntry), TimerRegistrationError> {
    let cancellation = Arc::new(GenerationCancellation::new());
    let cleanup_signal = Arc::clone(&cancellation);
    let registration = ctx
        .effect_sync(move || cleanup_signal.cancel())
        .map_err(|_| TimerRegistrationError::InactiveContext)?;
    Ok((cancellation, EffectEntry(Some(registration))))
}

fn prepare_one_shot(
    ctx: &Context,
    delay: Duration,
) -> Result<OneShotParts, TimerRegistrationError> {
    ctx.__generation_cleanup_admission()
        .map_err(|_| TimerRegistrationError::InactiveContext)?;
    let deadline = prepare_deadline(delay)?;
    let (cancellation, cleanup) = commit_cancellation(ctx)?;
    Ok((deadline, cancellation, cleanup))
}

type OneShotParts = (
    Pin<Box<tokio::time::Sleep>>,
    Arc<GenerationCancellation>,
    EffectEntry,
);

impl TimerExt for Context {
    fn timeout<F: Future>(
        &self,
        delay: Duration,
        work: F,
    ) -> Result<Timeout<F>, TimerRegistrationError> {
        self.__generation_cleanup_admission()
            .map_err(|_| TimerRegistrationError::InactiveContext)?;
        let deadline = prepare_deadline(delay)?;
        let (cancellation, cleanup) = commit_cancellation(self)?;
        Ok(Timeout {
            work: Some(Box::pin(work)),
            deadline: Some(deadline),
            cancellation,
            cleanup,
            terminated: false,
        })
    }

    fn sleep(&self, delay: Duration) -> Result<Sleep, TimerRegistrationError> {
        let (deadline, cancellation, cleanup) = prepare_one_shot(self, delay)?;
        Ok(Sleep {
            deadline: Some(deadline),
            cancellation,
            cleanup,
            terminated: false,
        })
    }

    fn interval(&self, period: Duration) -> Result<Interval, TimerRegistrationError> {
        if period.is_zero() {
            return Err(TimerRegistrationError::ZeroPeriod);
        }
        self.__generation_cleanup_admission()
            .map_err(|_| TimerRegistrationError::InactiveContext)?;
        let scheduler = prepare_interval(period)?;
        let (cancellation, cleanup) = commit_cancellation(self)?;
        Ok(Interval {
            scheduler: Some(scheduler),
            cancellation,
            cleanup,
            terminated: false,
        })
    }
}

struct EffectEntry(Option<EffectRegistration>);
impl EffectEntry {
    /// Claim natural completion/abandonment against the exact generation
    /// cleanup occurrence. A false result means generation cleanup already
    /// owns the occurrence, so no normal one-shot result may commit.
    fn disarm(&mut self) -> bool {
        self.0.take().is_some_and(EffectRegistration::disarm)
    }

    fn abandon(&mut self) {
        let _ = self.disarm();
    }
}
impl Drop for EffectEntry {
    fn drop(&mut self) {
        self.abandon();
    }
}

/// A construction-anchored fixed-phase timer stream.
///
/// On-time ticks yield `Ok(())`. If observation is late, missed ticks coalesce
/// to at most one overdue item and the next deadline remains on the original
/// construction-time phase. Generation cancellation yields exactly one
/// `Err(TimerCancelled)` and then terminates the stream. Dropping the Interval
/// abandons it without emitting a value.
pub struct Interval {
    scheduler: Option<tokio::time::Interval>,
    cancellation: Arc<GenerationCancellation>,
    cleanup: EffectEntry,
    terminated: bool,
}

impl Stream for Interval {
    type Item = Result<(), TimerCancelled>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.terminated {
            return Poll::Ready(None);
        }
        if this.cancellation.is_cancelled(cx) {
            this.terminated = true;
            this.cleanup.abandon();
            this.scheduler.take();
            return Poll::Ready(Some(Err(TimerCancelled)));
        }

        let scheduler = this
            .scheduler
            .as_mut()
            .expect("live Interval has scheduler");
        match Pin::new(scheduler).poll_tick(cx) {
            Poll::Pending => {
                if this.cancellation.is_cancelled(cx) {
                    this.terminated = true;
                    this.cleanup.abandon();
                    this.scheduler.take();
                    Poll::Ready(Some(Err(TimerCancelled)))
                } else {
                    Poll::Pending
                }
            }
            Poll::Ready(_) => {
                if this.cancellation.is_cancelled(cx) {
                    this.terminated = true;
                    this.cleanup.abandon();
                    this.scheduler.take();
                    Poll::Ready(Some(Err(TimerCancelled)))
                } else {
                    Poll::Ready(Some(Ok(())))
                }
            }
        }
    }
}

/// The normal terminal outcome of a [`Timeout`].
#[derive(Debug)]
pub enum TimeoutOutcome<T> {
    /// The owned work became ready before the pinned deadline elapsed.
    Completed(T),
    /// The pinned deadline elapsed before work readiness could commit.
    Elapsed,
}

struct GenerationCancellation {
    cancelled: AtomicBool,
    waker: AtomicWaker,
}

impl GenerationCancellation {
    fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            waker: AtomicWaker::new(),
        }
    }

    fn cancel(&self) {
        if !self.cancelled.swap(true, Ordering::AcqRel) {
            self.waker.wake();
        }
    }

    fn is_cancelled(&self, cx: &TaskContext<'_>) -> bool {
        if self.cancelled.load(Ordering::Acquire) {
            return true;
        }
        self.waker.register(cx.waker());
        self.cancelled.load(Ordering::Acquire)
    }
}

/// One lazy caller Future racing a deadline pinned at construction.
///
/// The work Future is never polled by construction or generation cleanup. A
/// ready work value commits only after the deadline is re-polled and remains
/// unelapsed; lifecycle cancellation is reported separately from normal expiry.
/// Dropping abandons the operation, disarms cleanup when possible, and drops
/// caller work in the caller's frame without emitting a cancellation result.
pub struct Timeout<F: Future> {
    work: Option<Pin<Box<F>>>,
    deadline: Option<Pin<Box<tokio::time::Sleep>>>,
    cancellation: Arc<GenerationCancellation>,
    cleanup: EffectEntry,
    terminated: bool,
}

impl<F: Future> Future for Timeout<F> {
    type Output = Result<TimeoutOutcome<F::Output>, TimerCancelled>;

    fn poll(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        assert!(!this.terminated, "polled Timeout after completion");
        let result = {
            let work = this.work.as_mut().expect("live Timeout owns work");
            let deadline = this.deadline.as_mut().expect("live Timeout owns deadline");
            poll_timeout(
                work.as_mut(),
                deadline.as_mut(),
                &this.cancellation,
                &mut this.cleanup,
                cx,
            )
        };
        if result.is_ready() {
            this.terminated = true;
            // Terminal work/deadline destruction happens in the caller's poll
            // frame. Generation cleanup owns only the cancellation signal.
            this.work.take();
            this.deadline.take();
        }
        result
    }
}

fn poll_timeout<F, D>(
    mut work: Pin<&mut F>,
    mut deadline: Pin<&mut D>,
    cancellation: &GenerationCancellation,
    cleanup: &mut EffectEntry,
    cx: &mut TaskContext<'_>,
) -> Poll<Result<TimeoutOutcome<F::Output>, TimerCancelled>>
where
    F: Future,
    D: Future<Output = ()>,
{
    if cancellation.is_cancelled(cx) {
        cleanup.abandon();
        return Poll::Ready(Err(TimerCancelled));
    }

    if deadline.as_mut().poll(cx).is_ready() {
        if cancellation.is_cancelled(cx) {
            cleanup.abandon();
            return Poll::Ready(Err(TimerCancelled));
        }
        return if cleanup.disarm() {
            Poll::Ready(Ok(TimeoutOutcome::Elapsed))
        } else {
            Poll::Ready(Err(TimerCancelled))
        };
    }

    match work.as_mut().poll(cx) {
        Poll::Pending => {
            if cancellation.is_cancelled(cx) {
                cleanup.abandon();
                Poll::Ready(Err(TimerCancelled))
            } else {
                Poll::Pending
            }
        }
        Poll::Ready(output) => {
            if cancellation.is_cancelled(cx) {
                cleanup.abandon();
                return Poll::Ready(Err(TimerCancelled));
            }

            // Work readiness is provisional until the exact pinned deadline
            // is checked again. This closes work-poll boundary crossings.
            if deadline.as_mut().poll(cx).is_ready() {
                if cancellation.is_cancelled(cx) {
                    cleanup.abandon();
                    return Poll::Ready(Err(TimerCancelled));
                }
                if cleanup.disarm() {
                    Poll::Ready(Ok(TimeoutOutcome::Elapsed))
                } else {
                    Poll::Ready(Err(TimerCancelled))
                }
            } else if cancellation.is_cancelled(cx) {
                cleanup.abandon();
                Poll::Ready(Err(TimerCancelled))
            } else if cleanup.disarm() {
                Poll::Ready(Ok(TimeoutOutcome::Completed(output)))
            } else {
                Poll::Ready(Err(TimerCancelled))
            }
        }
    }
}

/// One complete one-shot timer operation.
///
/// Its monotonic deadline is pinned at successful construction. Generation
/// cancellation wins any uncommitted expiry; natural completion and Drop each
/// arbitrate the same exact cleanup occurrence, and Drop emits no result.
pub struct Sleep {
    deadline: Option<Pin<Box<tokio::time::Sleep>>>,
    cancellation: Arc<GenerationCancellation>,
    cleanup: EffectEntry,
    terminated: bool,
}

impl Future for Sleep {
    type Output = Result<(), TimerCancelled>;

    fn poll(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        assert!(!this.terminated, "polled Sleep after completion");

        let deadline = this.deadline.as_mut().expect("live Sleep owns deadline");
        let result = if this.cancellation.is_cancelled(cx) {
            this.cleanup.abandon();
            Poll::Ready(Err(TimerCancelled))
        } else if deadline.as_mut().poll(cx).is_ready() {
            if this.cancellation.is_cancelled(cx) {
                this.cleanup.abandon();
                Poll::Ready(Err(TimerCancelled))
            } else if this.cleanup.disarm() {
                Poll::Ready(Ok(()))
            } else {
                // Generation cleanup claimed this exact occurrence before
                // natural completion could disarm it, so cancellation owns
                // the boundary even if its signal has not executed yet.
                Poll::Ready(Err(TimerCancelled))
            }
        } else if this.cancellation.is_cancelled(cx) {
            this.cleanup.abandon();
            Poll::Ready(Err(TimerCancelled))
        } else {
            Poll::Pending
        };

        if result.is_ready() {
            this.terminated = true;
            this.deadline.take();
        }
        result
    }
}

#[cfg(test)]
mod timeout_arbitration_tests {
    use super::*;
    use std::cell::Cell;
    use std::task::Waker;

    struct BoundaryWork<'a>(&'a Cell<bool>);
    impl Future for BoundaryWork<'_> {
        type Output = ();
        fn poll(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<()> {
            self.0.set(true);
            Poll::Ready(())
        }
    }

    struct BoundaryDeadline<'a>(&'a Cell<bool>);
    impl Future for BoundaryDeadline<'_> {
        type Output = ();
        fn poll(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<()> {
            if self.0.get() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }
    }

    #[test]
    fn work_ready_is_rechecked_against_deadline_before_commit() {
        let boundary = Cell::new(false);
        let mut work = BoundaryWork(&boundary);
        let mut deadline = BoundaryDeadline(&boundary);
        let cancellation = GenerationCancellation::new();
        let ctx = Context::new();
        let mut cleanup = EffectEntry(Some(ctx.effect_sync(|| {}).unwrap()));
        let waker = Waker::noop();
        let mut cx = TaskContext::from_waker(waker);

        let result = poll_timeout(
            Pin::new(&mut work),
            Pin::new(&mut deadline),
            &cancellation,
            &mut cleanup,
            &mut cx,
        );
        assert!(matches!(result, Poll::Ready(Ok(TimeoutOutcome::Elapsed))));
    }
}
