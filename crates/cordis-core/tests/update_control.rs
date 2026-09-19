//! Issue 35 typed same-Fiber update-control conformance evidence.

use cordis_core::event::{InvocationFailureKind, ListenerOptions, around, mapper, mapper_sync};
use cordis_core::lifecycle::{
    FiberState, PluginFailureKind, UpdateError, UpdateNext, UpdateOutcome,
};
use cordis_core::{Context, InjectSpec, Plugin, PreparedChange, PreparedPlugin};
use parking_lot::Mutex;
use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;

#[derive(Clone)]
struct Probe {
    seen: Arc<Mutex<Vec<u8>>>,
}

impl Plugin for Probe {
    type Config = u8;
    type Input = u8;
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, config: u8) -> Result<u8, Infallible> {
        Ok(config)
    }

    fn apply(
        &self,
        _ctx: Context,
        input: &u8,
    ) -> impl Future<Output = Result<(), Infallible>> + Send {
        self.seen.lock().push(*input);
        std::future::ready(Ok(()))
    }
}

#[tokio::test]
async fn typed_mapper_transforms_before_one_commit() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let _policy = ctx
        .on_update::<Probe, _>(
            mapper_sync::<Probe, _>(|_ctx, value| Ok::<_, Infallible>(value + 1)),
            ListenerOptions::default(),
        )
        .unwrap();
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(Probe { seen: seen.clone() }, 1))
        .await
        .unwrap();
    let id = fiber_handle.id();

    let outcome = fiber_handle
        .update(PreparedChange::from_input::<Probe>(2))
        .await
        .unwrap();
    assert_eq!(outcome, UpdateOutcome::Committed(FiberState::Active));
    assert_eq!(fiber_handle.id(), id);
    assert_eq!(*seen.lock(), vec![1, 3]);
}

#[tokio::test]
async fn around_can_veto_without_reaching_private_tail() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let _policy = ctx
        .on_update::<Probe, _>(
            around::<Probe, _>(|_ctx, value, _next: UpdateNext<Probe>| async move {
                Ok::<_, Infallible>(value)
            }),
            ListenerOptions::default(),
        )
        .unwrap();
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(Probe { seen: seen.clone() }, 1))
        .await
        .unwrap();

    let outcome = fiber_handle
        .update(PreparedChange::from_input::<Probe>(9))
        .await
        .unwrap();
    assert_eq!(outcome, UpdateOutcome::Vetoed);
    assert_eq!(*seen.lock(), vec![1]);
}

#[tokio::test]
async fn routing_is_target_scoped_and_global_widens_it() {
    let root = Context::new();
    let branch = root.with_child_scope();
    let sibling = root.with_child_scope();
    let calls = Arc::new(Mutex::new(Vec::new()));

    let branch_calls = calls.clone();
    let _ancestor = branch
        .on_update::<Probe, _>(
            mapper_sync::<Probe, _>(move |_ctx, value| {
                branch_calls.lock().push("ancestor");
                Ok::<_, Infallible>(value)
            }),
            ListenerOptions::default(),
        )
        .unwrap();
    let sibling_calls = calls.clone();
    let _sibling = sibling
        .on_update::<Probe, _>(
            mapper_sync::<Probe, _>(move |_ctx, value| {
                sibling_calls.lock().push("sibling");
                Ok::<_, Infallible>(value)
            }),
            ListenerOptions::default(),
        )
        .unwrap();
    let global_calls = calls.clone();
    let _global = sibling
        .on_update::<Probe, _>(
            mapper_sync::<Probe, _>(move |_ctx, value| {
                global_calls.lock().push("global");
                Ok::<_, Infallible>(value)
            }),
            ListenerOptions::default().global(),
        )
        .unwrap();

    let seen = Arc::new(Mutex::new(Vec::new()));
    let fiber_handle = branch
        .spawn(PreparedPlugin::from_input(Probe { seen }, 1))
        .await
        .unwrap();
    fiber_handle
        .update(PreparedChange::from_input::<Probe>(2))
        .await
        .unwrap();
    assert_eq!(*calls.lock(), vec!["ancestor", "global"]);
}

#[tokio::test]
async fn wrong_contract_is_precommit_and_typed() {
    struct Other;
    impl Plugin for Other {
        type Config = ();
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = Infallible;
        fn prepare(&self, _: ()) -> Result<(), Infallible> {
            Ok(())
        }
        fn apply(&self, _: Context, _: &()) -> impl Future<Output = Result<(), Infallible>> + Send {
            std::future::ready(Ok(()))
        }
    }
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(Probe { seen: seen.clone() }, 1))
        .await
        .unwrap();
    let id = fiber_handle.id();
    let err = fiber_handle
        .update(PreparedChange::from_input::<Other>(()))
        .await
        .unwrap_err();
    assert!(matches!(err, UpdateError::PluginContractMismatch));
    assert_eq!(fiber_handle.id(), id);
    assert_eq!(*seen.lock(), vec![1]);
}

#[tokio::test]
async fn private_tail_is_provisional_until_outer_control_returns() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let tail_seen = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let t = tail_seen.clone();
    let r = release.clone();
    let _policy = ctx
        .on_update::<Probe, _>(
            around::<Probe, _>(move |_ctx, value, next: UpdateNext<Probe>| {
                let t = t.clone();
                let r = r.clone();
                async move {
                    let value = next.call(value).await?;
                    t.notify_one();
                    r.notified().await;
                    Ok::<_, cordis_core::event::InvocationFailure>(value + 1)
                }
            }),
            ListenerOptions::default(),
        )
        .unwrap();
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(Probe { seen: seen.clone() }, 1))
        .await
        .unwrap();
    let task = tokio::spawn({
        let fiber_handle = fiber_handle.clone();
        async move {
            fiber_handle
                .update(PreparedChange::from_input::<Probe>(2))
                .await
        }
    });
    tail_seen.notified().await;
    assert_eq!(*seen.lock(), vec![1], "tail reach is not lifecycle commit");
    assert_eq!(fiber_handle.state(), FiberState::Active);
    release.notify_one();
    assert_eq!(
        task.await.unwrap().unwrap(),
        UpdateOutcome::Committed(FiberState::Active)
    );
    assert_eq!(*seen.lock(), vec![1, 3]);
}

#[tokio::test]
async fn outer_around_can_recover_downstream_failure_after_tail() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let _outer = ctx
        .on_update::<Probe, _>(
            around::<Probe, _>(|_ctx, value, next: UpdateNext<Probe>| async move {
                match next.call(value).await {
                    Ok(value) => Ok::<_, Infallible>(value),
                    Err(_) => Ok(value + 1),
                }
            }),
            ListenerOptions::default(),
        )
        .unwrap();
    let _inner = ctx
        .on_update::<Probe, _>(
            around::<Probe, _>(|_ctx, value, next: UpdateNext<Probe>| async move {
                let _ = next
                    .call(value)
                    .await
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
                Err::<u8, _>(std::io::Error::other("policy rejected after tail"))
            }),
            ListenerOptions::default(),
        )
        .unwrap();
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(Probe { seen: seen.clone() }, 1))
        .await
        .unwrap();
    assert_eq!(
        fiber_handle
            .update(PreparedChange::from_input::<Probe>(2))
            .await
            .unwrap(),
        UpdateOutcome::Committed(FiberState::Active)
    );
    assert_eq!(*seen.lock(), vec![1, 3]);
}

#[tokio::test]
async fn close_during_awaited_control_reports_admission_lost() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let e = entered.clone();
    let r = release.clone();
    let _policy = ctx
        .on_update::<Probe, _>(
            around::<Probe, _>(move |_ctx, value, next: UpdateNext<Probe>| {
                let e = e.clone();
                let r = r.clone();
                async move {
                    e.notify_one();
                    r.notified().await;
                    next.call(value).await
                }
            }),
            ListenerOptions::default(),
        )
        .unwrap();
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(Probe { seen: seen.clone() }, 1))
        .await
        .unwrap();
    let task = tokio::spawn({
        let fiber_handle = fiber_handle.clone();
        async move {
            fiber_handle
                .update(PreparedChange::from_input::<Probe>(2))
                .await
        }
    });
    entered.notified().await;
    fiber_handle.dispose().await.unwrap();
    release.notify_one();
    assert!(matches!(
        task.await.unwrap(),
        Err(UpdateError::AdmissionLost)
    ));
    assert_eq!(*seen.lock(), vec![1]);
}

#[derive(Clone)]
struct BlockingProbe {
    seen: Arc<Mutex<Vec<u8>>>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
}
impl Plugin for BlockingProbe {
    type Config = u8;
    type Input = u8;
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, v: u8) -> Result<u8, Infallible> {
        Ok(v)
    }
    async fn apply(&self, _: Context, v: &u8) -> Result<(), Infallible> {
        self.seen.lock().push(*v);
        if *v == 2 {
            self.entered.notify_one();
            self.release.notified().await;
        }
        Ok(())
    }
}

#[tokio::test]
async fn cancelling_postcommit_waiter_does_not_cancel_update_owner() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(
            BlockingProbe {
                seen: seen.clone(),
                entered: entered.clone(),
                release: release.clone(),
            },
            1,
        ))
        .await
        .unwrap();
    let task = tokio::spawn({
        let fiber_handle = fiber_handle.clone();
        async move {
            fiber_handle
                .update(PreparedChange::from_input::<BlockingProbe>(2))
                .await
        }
    });
    entered.notified().await;
    task.abort();
    release.notify_one();
    assert_eq!(fiber_handle.ready().await.unwrap(), FiberState::Active);
    assert_eq!(*seen.lock(), vec![1, 2]);
}

#[tokio::test]
async fn committed_update_serializes_terminal_dispose_behind_its_barrier() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(
            BlockingProbe {
                seen: seen.clone(),
                entered: entered.clone(),
                release: release.clone(),
            },
            1,
        ))
        .await
        .unwrap();

    let update = tokio::spawn({
        let fiber_handle = fiber_handle.clone();
        async move {
            fiber_handle
                .update(PreparedChange::from_input::<BlockingProbe>(2))
                .await
        }
    });
    entered.notified().await;

    let mut dispose = Box::pin(fiber_handle.dispose());
    assert!(
        futures::poll!(&mut dispose).is_pending(),
        "terminal dispose must not pass a committed update that still owns the lifecycle slot"
    );

    release.notify_one();
    assert_eq!(
        update.await.unwrap().unwrap(),
        UpdateOutcome::Committed(FiberState::Active)
    );
    dispose.await.unwrap();

    assert_eq!(fiber_handle.state(), FiberState::Disposed);
    assert_eq!(*seen.lock(), vec![1, 2]);
}

#[tokio::test]
async fn committed_update_serializes_era_swap_until_update_quiescence() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let source = ctx
        .spawn(PreparedPlugin::from_input(
            BlockingProbe {
                seen: seen.clone(),
                entered: entered.clone(),
                release: release.clone(),
            },
            1,
        ))
        .await
        .unwrap();
    let source_id = source.id();

    let update = tokio::spawn({
        let source = source.clone();
        async move {
            source
                .update(PreparedChange::from_input::<BlockingProbe>(2))
                .await
        }
    });
    entered.notified().await;

    let mut swap = Box::pin(source.era_swap(PreparedChange::from_input::<BlockingProbe>(3)));
    assert!(
        futures::poll!(&mut swap).is_pending(),
        "era swap must not claim a source while a committed update owns the lifecycle slot"
    );

    release.notify_one();
    assert_eq!(
        update.await.unwrap().unwrap(),
        UpdateOutcome::Committed(FiberState::Active)
    );
    let successor = swap.await.unwrap();

    assert_ne!(successor.id(), source_id);
    assert_eq!(source.state(), FiberState::Disposed);
    assert_eq!(successor.state(), FiberState::Active);
    assert_eq!(*seen.lock(), vec![1, 2, 3]);

    successor.dispose().await.unwrap();
}

#[tokio::test]
async fn concurrent_updates_commit_in_postcontrol_admission_order() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let e = entered.clone();
    let r = release.clone();
    let _policy = ctx
        .on_update::<Probe, _>(
            mapper::<Probe, _>(move |_ctx, value| {
                let e = e.clone();
                let r = r.clone();
                async move {
                    if value == 2 {
                        e.notify_one();
                        r.notified().await;
                    }
                    Ok::<_, Infallible>(value)
                }
            }),
            ListenerOptions::default(),
        )
        .unwrap();
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(Probe { seen: seen.clone() }, 1))
        .await
        .unwrap();
    let a = tokio::spawn({
        let fiber_handle = fiber_handle.clone();
        async move {
            fiber_handle
                .update(PreparedChange::from_input::<Probe>(2))
                .await
        }
    });
    entered.notified().await;
    let b = tokio::spawn({
        let fiber_handle = fiber_handle.clone();
        async move {
            fiber_handle
                .update(PreparedChange::from_input::<Probe>(3))
                .await
        }
    });
    assert_eq!(
        b.await.unwrap().unwrap(),
        UpdateOutcome::Committed(FiberState::Active)
    );
    release.notify_one();
    assert_eq!(
        a.await.unwrap().unwrap(),
        UpdateOutcome::Committed(FiberState::Active)
    );
    assert_eq!(*seen.lock(), vec![1, 3, 2]);
}

#[derive(Clone)]
struct FailingProbe {
    seen: Arc<Mutex<Vec<u8>>>,
}
impl Plugin for FailingProbe {
    type Config = u8;
    type Input = u8;
    type PrepareError = Infallible;
    type ApplyError = std::io::Error;
    fn prepare(&self, v: u8) -> Result<u8, Infallible> {
        Ok(v)
    }
    async fn apply(&self, _: Context, v: &u8) -> Result<(), std::io::Error> {
        self.seen.lock().push(*v);
        if *v == 2 {
            Err(std::io::Error::other("apply two"))
        } else {
            Ok(())
        }
    }
}

struct PanickingApplyProbe {
    seen: Arc<Mutex<Vec<u8>>>,
}

impl Plugin for PanickingApplyProbe {
    type Config = u8;
    type Input = u8;
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, value: u8) -> Result<u8, Infallible> {
        Ok(value)
    }

    async fn apply(&self, _: Context, value: &u8) -> Result<(), Infallible> {
        self.seen.lock().push(*value);
        if *value == 2 {
            panic!("update apply panic")
        }
        Ok(())
    }
}

#[tokio::test]
async fn postcommit_update_apply_panic_is_contained_and_keeps_the_new_input() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(
            PanickingApplyProbe { seen: seen.clone() },
            1,
        ))
        .await
        .unwrap();
    let id = fiber_handle.id();

    let error = fiber_handle
        .update(PreparedChange::from_input::<PanickingApplyProbe>(2))
        .await
        .expect_err("committed update apply panics");
    let UpdateError::Apply(failure) = error else {
        panic!("expected normalized postcommit apply panic: {error:?}")
    };
    assert_eq!(failure.kind(), PluginFailureKind::Panic);
    assert!(failure.diagnostic().contains("update apply panic"));
    assert_eq!(fiber_handle.id(), id);
    assert_eq!(fiber_handle.state(), FiberState::Failed);

    assert!(fiber_handle.restart().await.is_err());
    assert_eq!(
        *seen.lock(),
        vec![1, 2, 2],
        "restart retries the committed replacement input rather than restoring the old input"
    );
}

#[tokio::test]
async fn postcommit_apply_failure_is_invisible_to_control_and_candidate_is_retained() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let _policy = ctx
        .on_update::<FailingProbe, _>(
            mapper_sync::<FailingProbe, _>(move |_ctx, v| {
                h.fetch_add(1, Ordering::SeqCst);
                Ok::<_, Infallible>(v)
            }),
            ListenerOptions::default(),
        )
        .unwrap();
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(
            FailingProbe { seen: seen.clone() },
            1,
        ))
        .await
        .unwrap();
    assert!(matches!(
        fiber_handle
            .update(PreparedChange::from_input::<FailingProbe>(2))
            .await,
        Err(UpdateError::Apply(_))
    ));
    assert_eq!(fiber_handle.state(), FiberState::Failed);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert!(fiber_handle.restart().await.is_err());
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "restart/failed apply never re-enters update control"
    );
    assert_eq!(
        *seen.lock(),
        vec![1, 2, 2],
        "new input value remains authoritative after failed apply"
    );
}

#[tokio::test]
async fn cancelling_during_precommit_control_commits_nothing() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let e = entered.clone();
    let r = release.clone();
    let _policy = ctx
        .on_update::<Probe, _>(
            mapper::<Probe, _>(move |_ctx, v| {
                let e = e.clone();
                let r = r.clone();
                async move {
                    e.notify_one();
                    r.notified().await;
                    Ok::<_, Infallible>(v)
                }
            }),
            ListenerOptions::default(),
        )
        .unwrap();
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(Probe { seen: seen.clone() }, 1))
        .await
        .unwrap();
    let task = tokio::spawn({
        let fiber_handle = fiber_handle.clone();
        async move {
            fiber_handle
                .update(PreparedChange::from_input::<Probe>(2))
                .await
        }
    });
    entered.notified().await;
    task.abort();
    release.notify_one();
    tokio::task::yield_now().await;
    assert_eq!(fiber_handle.state(), FiberState::Active);
    assert_eq!(*seen.lock(), vec![1]);
}

#[tokio::test]
async fn era_swap_never_invokes_update_control() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let _policy = ctx
        .on_update::<Probe, _>(
            mapper_sync::<Probe, _>(move |_ctx, v| {
                h.fetch_add(1, Ordering::SeqCst);
                Ok::<_, Infallible>(v)
            }),
            ListenerOptions::default(),
        )
        .unwrap();
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(Probe { seen: seen.clone() }, 1))
        .await
        .unwrap();
    let old_id = fiber_handle.id();
    let replacement = fiber_handle
        .era_swap(PreparedChange::from_input::<Probe>(2))
        .await
        .unwrap();
    assert_ne!(replacement.id(), old_id);
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    assert_eq!(*seen.lock(), vec![1, 2]);
}

#[tokio::test]
async fn unrecovered_control_error_preserves_old_generation() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let _policy = ctx
        .on_update::<Probe, _>(
            mapper_sync::<Probe, _>(|_ctx, _v| Err::<u8, _>(std::io::Error::other("no"))),
            ListenerOptions::default(),
        )
        .unwrap();
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(Probe { seen: seen.clone() }, 1))
        .await
        .unwrap();
    let id = fiber_handle.id();
    assert!(matches!(
        fiber_handle
            .update(PreparedChange::from_input::<Probe>(2))
            .await,
        Err(UpdateError::Control(_))
    ));
    assert_eq!(fiber_handle.id(), id);
    assert_eq!(fiber_handle.state(), FiberState::Active);
    assert_eq!(*seen.lock(), vec![1]);
}

#[derive(Clone)]
struct ReentrantProbe {
    fiber_handle: Arc<Mutex<Option<cordis_core::FiberHandle>>>,
    result: Arc<Mutex<Option<Result<UpdateOutcome, UpdateError>>>>,
}
impl Plugin for ReentrantProbe {
    type Config = u8;
    type Input = u8;
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, v: u8) -> Result<u8, Infallible> {
        Ok(v)
    }
    async fn apply(&self, _: Context, _: &u8) -> Result<(), Infallible> {
        let fiber_handle = self.fiber_handle.lock().clone();
        if let Some(fiber_handle) = fiber_handle {
            let outcome = fiber_handle
                .update(PreparedChange::from_input::<ReentrantProbe>(2))
                .await;
            *self.result.lock() = Some(outcome);
        }
        Ok(())
    }
}

#[tokio::test]
async fn same_fiber_update_recursion_is_refused_before_control() {
    let ctx = Context::new();
    let fiber_handle_cell = Arc::new(Mutex::new(None));
    let result = Arc::new(Mutex::new(None));
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let _policy = ctx
        .on_update::<ReentrantProbe, _>(
            mapper_sync::<ReentrantProbe, _>(move |_ctx, v| {
                h.fetch_add(1, Ordering::SeqCst);
                Ok::<_, Infallible>(v)
            }),
            ListenerOptions::default(),
        )
        .unwrap();
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(
            ReentrantProbe {
                fiber_handle: fiber_handle_cell.clone(),
                result: result.clone(),
            },
            1,
        ))
        .await
        .unwrap();
    *fiber_handle_cell.lock() = Some(fiber_handle.clone());
    fiber_handle.restart().await.unwrap();
    let outcome = result
        .lock()
        .take()
        .expect("restart apply attempted update");
    match outcome {
        Err(UpdateError::Recursion(recursion)) => {
            assert_eq!(
                recursion.operation(),
                cordis_core::lifecycle::LifecycleOperation::Update
            );
            assert_eq!(recursion.fiber_id(), &fiber_handle.id());
        }
        other => panic!("expected typed update recursion refusal, got {other:?}"),
    }
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "recursion refusal precedes update control"
    );
}

#[derive(Clone)]
struct PendingProbe {
    applies: Arc<AtomicUsize>,
}
impl Plugin for PendingProbe {
    type Config = u8;
    type Input = u8;
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, v: u8) -> Result<u8, Infallible> {
        Ok(v)
    }
    fn inject(&self) -> InjectSpec {
        InjectSpec::none().require("issue35/missing")
    }
    async fn apply(&self, _: Context, _: &u8) -> Result<(), Infallible> {
        self.applies.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn accepted_update_can_commit_to_stable_pending_without_apply() {
    let ctx = Context::new();
    let applies = Arc::new(AtomicUsize::new(0));
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(
            PendingProbe {
                applies: applies.clone(),
            },
            1,
        ))
        .await
        .unwrap();
    assert_eq!(fiber_handle.ready().await.unwrap(), FiberState::Pending);
    let id = fiber_handle.id();
    assert_eq!(
        fiber_handle
            .update(PreparedChange::from_input::<PendingProbe>(2))
            .await
            .unwrap(),
        UpdateOutcome::Committed(FiberState::Pending)
    );
    assert_eq!(fiber_handle.id(), id);
    assert_eq!(applies.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn prepend_and_once_fix_typed_transformation_order() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let _append = ctx
        .on_update::<Probe, _>(
            mapper_sync::<Probe, _>(|_, v| Ok::<_, Infallible>(v + 1)),
            ListenerOptions::default(),
        )
        .unwrap();
    let _prepend_once = ctx
        .on_update::<Probe, _>(
            mapper_sync::<Probe, _>(|_, v| Ok::<_, Infallible>(v * 10)),
            ListenerOptions::default().prepend().once(),
        )
        .unwrap();
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(Probe { seen: seen.clone() }, 1))
        .await
        .unwrap();
    fiber_handle
        .update(PreparedChange::from_input::<Probe>(2))
        .await
        .unwrap();
    fiber_handle
        .update(PreparedChange::from_input::<Probe>(2))
        .await
        .unwrap();
    assert_eq!(*seen.lock(), vec![1, 21, 3]);
}

#[tokio::test]
async fn mapper_panic_is_contained_precommit_and_preserves_the_old_generation() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let _registration = ctx
        .on_update::<Probe, _>(
            mapper_sync::<Probe, _>(|_, _v| -> Result<u8, Infallible> { panic!("mapper panic") }),
            ListenerOptions::default(),
        )
        .unwrap();
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(Probe { seen: seen.clone() }, 1))
        .await
        .unwrap();
    let id = fiber_handle.id();

    match fiber_handle
        .update(PreparedChange::from_input::<Probe>(2))
        .await
    {
        Err(UpdateError::Control(failure)) => {
            assert_eq!(failure.kind(), InvocationFailureKind::Panic);
            assert!(failure.diagnostic().contains("mapper panic"));
            assert!(failure.registration_id().is_some());
        }
        other => panic!("expected contained mapper panic, got {other:?}"),
    }

    assert_eq!(fiber_handle.id(), id);
    assert_eq!(fiber_handle.state(), FiberState::Active);
    assert_eq!(*seen.lock(), vec![1]);
}

#[tokio::test]
async fn mapper_failure_is_correlated_to_a_claimed_update_occurrence() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let _registration = ctx
        .on_update::<Probe, _>(
            mapper_sync::<Probe, _>(|_, _v| Err::<u8, _>(std::io::Error::other("mapper failed"))),
            ListenerOptions::default(),
        )
        .unwrap();
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(Probe { seen: seen.clone() }, 1))
        .await
        .unwrap();
    match fiber_handle
        .update(PreparedChange::from_input::<Probe>(2))
        .await
    {
        Err(UpdateError::Control(failure)) => assert!(failure.registration_id().is_some()),
        other => panic!("expected correlated mapper control failure, got {other:?}"),
    }
    assert_eq!(*seen.lock(), vec![1]);
}
