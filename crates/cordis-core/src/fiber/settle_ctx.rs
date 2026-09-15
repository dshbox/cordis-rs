//! Exact-allocation attribution for lifecycle-recursion refusal.
//!
//! `bracket` records every Fiber whose apply/settle or teardown body is running
//! on this task. `run_task` records the Fiber generation that owns a
//! `Context::run` task. Correctness compares the concrete allocation itself;
//! FiberId is used only in the typed public diagnostic.

use std::cell::RefCell;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use super::{Fiber, LifecycleOperation, LifecycleRecursion};

tokio::task_local! {
    static SETTLE_CTX: RefCell<SettleCtx>;
}

/// One exact-allocation attribution frame shared across task boundaries.
/// The source scope deactivates the frame when it exits, so a transferred
/// snapshot cannot keep a stale recursion refusal alive after the dependency
/// that made the wait unsafe has ended.
#[derive(Clone)]
struct AttributionFrame {
    fiber: Weak<Fiber>,
    active: Arc<AtomicBool>,
}

impl AttributionFrame {
    fn new(fiber: &Arc<Fiber>) -> Self {
        Self {
            fiber: Arc::downgrade(fiber),
            active: Arc::new(AtomicBool::new(true)),
        }
    }

    fn is_active_for(&self, target: &Fiber) -> bool {
        self.active.load(Ordering::SeqCst)
            && std::ptr::eq(self.fiber.as_ptr(), target as *const Fiber)
    }

    fn deactivate(&self) {
        self.active.store(false, Ordering::SeqCst);
    }
}

/// Exact allocation facts carried by one task. The source scope guard pins each
/// named Fiber while its frame is live; transferred snapshots keep only weak
/// identity so they do not extend allocation lifetime after that scope ends.
#[derive(Clone, Default)]
pub(crate) struct SettleCtx {
    settling: Vec<AttributionFrame>,
    run_owner: Option<AttributionFrame>,
}

fn run_task_wait_is_doomed(fiber: &Fiber, operation: LifecycleOperation) -> bool {
    match operation {
        LifecycleOperation::Ready | LifecycleOperation::WaitState => {
            fiber.draining.load(Ordering::SeqCst)
        }
        LifecycleOperation::Restart
        | LifecycleOperation::Update
        | LifecycleOperation::EraSwap
        | LifecycleOperation::Dispose
        | LifecycleOperation::RemovePlugins => true,
    }
}

fn is_doomed(fiber: &Fiber, operation: LifecycleOperation) -> bool {
    let (in_settle, owns_run_task) = match SETTLE_CTX.try_with(|cell| {
        let cell = cell.borrow();
        (
            cell.settling
                .iter()
                .any(|candidate| candidate.is_active_for(fiber)),
            cell.run_owner
                .as_ref()
                .is_some_and(|candidate| candidate.is_active_for(fiber)),
        )
    }) {
        Ok(facts) => facts,
        Err(_) => return false,
    };
    in_settle || (owns_run_task && run_task_wait_is_doomed(fiber, operation))
}

pub(crate) fn refuse_recursion(
    fiber: &Fiber,
    operation: LifecycleOperation,
) -> std::result::Result<(), LifecycleRecursion> {
    if is_doomed(fiber, operation) {
        Err(LifecycleRecursion::new(operation, fiber.id().clone()))
    } else {
        Ok(())
    }
}

/// Capture the caller's exact allocation attribution before work changes task.
/// Framework continuations and `Context::spawn_attributed` transfer this
/// snapshot across task boundaries.
pub(crate) fn capture_attribution() -> SettleCtx {
    SETTLE_CTX
        .try_with(|cell| cell.borrow().clone())
        .unwrap_or_default()
}

/// Scope work to previously captured attribution. Tokio task-locals are
/// otherwise intentionally not inherited by spawned tasks.
pub(crate) async fn with_attribution<R>(
    attribution: SettleCtx,
    work: impl Future<Output = R>,
) -> R {
    SETTLE_CTX.scope(RefCell::new(attribution), work).await
}

/// Cancellation/unwind guard for one live settle-attribution frame. Nested
/// scopes also pop their frame from the current task-local stack; transferred
/// copies observe the shared liveness bit instead.
struct SettleFrameGuard {
    frame: AttributionFrame,
    _allocation_pin: Arc<Fiber>,
    pop_nested: bool,
}

impl Drop for SettleFrameGuard {
    fn drop(&mut self) {
        self.frame.deactivate();
        if self.pop_nested {
            let _ = SETTLE_CTX.try_with(|cell| cell.borrow_mut().settling.pop());
        }
    }
}

/// Attribute one apply/settle or teardown body to `fiber` for exactly `pass`.
/// Nested bodies form a stack so an inner Fiber cannot hide an outer allocation.
pub(crate) async fn bracket<R>(fiber: Arc<Fiber>, pass: impl Future<Output = R>) -> R {
    let frame = AttributionFrame::new(&fiber);
    if SETTLE_CTX
        .try_with(|cell| cell.borrow_mut().settling.push(frame.clone()))
        .is_ok()
    {
        let _guard = SettleFrameGuard {
            frame,
            _allocation_pin: fiber,
            pop_nested: true,
        };
        pass.await
    } else {
        let guard_frame = frame.clone();
        SETTLE_CTX
            .scope(
                RefCell::new(SettleCtx {
                    settling: vec![frame],
                    run_owner: None,
                }),
                async move {
                    let _guard = SettleFrameGuard {
                        frame: guard_frame,
                        _allocation_pin: fiber,
                        pop_nested: false,
                    };
                    pass.await
                },
            )
            .await
    }
}

/// Scope one generation-owned `Context::run` body to its exact owning Fiber.
/// Mutators on that owner always self-wait; `ready`/`wait_state` do so only while
/// an active drain is joining the task. The task output is consumed here, so the
/// generation drain joins completion rather than retaining a user value.
pub(crate) async fn run_task<F>(fiber: Arc<Fiber>, task: F)
where
    F: Future,
{
    let frame = AttributionFrame::new(&fiber);
    let guard_frame = frame.clone();
    let _ = SETTLE_CTX
        .scope(
            RefCell::new(SettleCtx {
                settling: Vec::new(),
                run_owner: Some(frame),
            }),
            async move {
                let _guard = SettleFrameGuard {
                    frame: guard_frame,
                    _allocation_pin: fiber,
                    pop_nested: false,
                };
                task.await
            },
        )
        .await;
}

#[cfg(test)]
mod tests {
    use super::SettleCtx;
    use crate::fiber::{Fiber, LifecycleOperation};
    use std::cell::RefCell;
    use std::future::pending;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    const OPS: [LifecycleOperation; 7] = [
        LifecycleOperation::Ready,
        LifecycleOperation::WaitState,
        LifecycleOperation::Restart,
        LifecycleOperation::Update,
        LifecycleOperation::EraSwap,
        LifecycleOperation::Dispose,
        LifecycleOperation::RemovePlugins,
    ];
    const MUTATORS: [LifecycleOperation; 5] = [
        LifecycleOperation::Restart,
        LifecycleOperation::Update,
        LifecycleOperation::EraSwap,
        LifecycleOperation::Dispose,
        LifecycleOperation::RemovePlugins,
    ];

    fn fiber() -> Arc<Fiber> {
        Fiber::new("test")
    }

    fn refused(fiber: &Fiber, operation: LifecycleOperation) {
        let error = super::refuse_recursion(fiber, operation).unwrap_err();
        assert_eq!(error.operation(), operation);
        assert_eq!(error.fiber_id(), fiber.id());
    }

    #[tokio::test]
    async fn typed_refusal_table_is_exact_to_operation_and_allocation() {
        let a = fiber();
        let b = fiber();
        let c = fiber();
        for operation in OPS {
            super::refuse_recursion(&a, operation).unwrap();
        }

        super::bracket(a.clone(), async {
            for operation in OPS {
                refused(&a, operation);
                super::refuse_recursion(&b, operation).unwrap();
            }
        })
        .await;

        super::bracket(
            a.clone(),
            super::bracket(b.clone(), async {
                for operation in OPS {
                    refused(&a, operation);
                    refused(&b, operation);
                    super::refuse_recursion(&c, operation).unwrap();
                }
            }),
        )
        .await;

        super::run_task(a.clone(), async move {
            for operation in MUTATORS {
                refused(&a, operation);
                super::refuse_recursion(&b, operation).unwrap();
            }
            for operation in [LifecycleOperation::Ready, LifecycleOperation::WaitState] {
                super::refuse_recursion(&a, operation).unwrap();
            }
            a.draining.store(true, Ordering::SeqCst);
            for operation in [LifecycleOperation::Ready, LifecycleOperation::WaitState] {
                refused(&a, operation);
            }
            a.draining.store(false, Ordering::SeqCst);
        })
        .await;
    }

    #[tokio::test]
    async fn attribution_survives_lifecycle_death_by_allocation_identity() {
        let a = fiber();
        let id = a.id().clone();
        a.alive.store(false, Ordering::Relaxed);
        super::bracket(a.clone(), async {
            for operation in OPS {
                let error = super::refuse_recursion(&a, operation).unwrap_err();
                assert_eq!(error.operation(), operation);
                assert_eq!(error.fiber_id(), &id);
            }
        })
        .await;
    }

    #[tokio::test]
    async fn captured_attribution_transfers_exactly_without_leaking() {
        let a = fiber();
        let b = fiber();
        let probe = a.clone();
        super::bracket(a.clone(), async {
            let captured = super::capture_attribution();
            tokio::spawn(super::with_attribution(captured, async move {
                refused(&probe, LifecycleOperation::EraSwap);
                super::refuse_recursion(&b, LifecycleOperation::EraSwap).unwrap();
            }))
            .await
            .unwrap();
        })
        .await;
        super::refuse_recursion(&a, LifecycleOperation::EraSwap).unwrap();
    }

    #[tokio::test]
    async fn transferred_attribution_expires_with_its_source_scope() {
        let a = fiber();
        let probe = a.clone();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let (child_tx, child_rx) = tokio::sync::oneshot::channel();
        super::bracket(a.clone(), async {
            let captured = super::capture_attribution();
            let child = tokio::spawn(super::with_attribution(captured, async move {
                release_rx.await.unwrap();
                super::refuse_recursion(&probe, LifecycleOperation::Ready).unwrap();
            }));
            child_tx.send(child).unwrap();
        })
        .await;

        let child = child_rx.await.unwrap();
        release_tx.send(()).unwrap();
        child.await.unwrap();
        super::refuse_recursion(&a, LifecycleOperation::Ready).unwrap();
    }

    #[tokio::test]
    async fn cancelled_nested_bracket_pops_only_its_allocation() {
        let a = fiber();
        let b = fiber();
        super::bracket(a.clone(), async {
            {
                let nested = super::bracket(b.clone(), pending::<()>());
                tokio::pin!(nested);
                tokio::select! {
                    biased;
                    _ = &mut nested => unreachable!(),
                    _ = tokio::task::yield_now() => {}
                }
            }
            refused(&a, LifecycleOperation::Ready);
            super::refuse_recursion(&b, LifecycleOperation::Ready).unwrap();
        })
        .await;
    }

    #[tokio::test]
    async fn scopes_start_clean_and_raw_tokio_spawn_does_not_inherit_attribution() {
        let a = fiber();
        super::bracket(a.clone(), async {
            super::SETTLE_CTX.with(|cell: &RefCell<SettleCtx>| {
                let cell = cell.borrow();
                assert_eq!(cell.settling.len(), 1);
                assert!(cell.run_owner.is_none());
            });
            refused(&a, LifecycleOperation::Ready);
            let unrelated = a.clone();
            tokio::spawn(async move {
                super::refuse_recursion(&unrelated, LifecycleOperation::Ready).unwrap();
            })
            .await
            .unwrap();
        })
        .await;
        super::refuse_recursion(&a, LifecycleOperation::Ready).unwrap();

        let b = fiber();
        let probe = b.clone();
        super::run_task(b, async move {
            super::SETTLE_CTX.with(|cell: &RefCell<SettleCtx>| {
                let cell = cell.borrow();
                assert!(cell.settling.is_empty());
                assert!(
                    cell.run_owner.as_ref().is_some_and(|owner| std::ptr::eq(
                        owner.fiber.as_ptr(),
                        Arc::as_ptr(&probe)
                    ))
                );
            });
        })
        .await;
    }
}
