//! Issue 28 evidence for reliable Fiber state publication and ready/wait_state semantics.

mod common;

use cordis_core::effect::EffectRegistrationError;
use cordis_core::lifecycle::{PluginFailureKind, ReadyError, WaitStateError};
use cordis_core::{Context, FiberState, InjectSpec, Plugin, PreparedPlugin, Service};
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

#[derive(Debug)]
struct Versioned(u32);
impl Service for Versioned {
    const NAME: &'static str = "t28-versioned";
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct ApplyBoom(String);

fn prepared<P>(plugin: P) -> PreparedPlugin
where
    P: Plugin<Config = (), Input = (), PrepareError = Infallible>,
{
    PreparedPlugin::from_input(plugin, ())
}

struct VersionedDependent {
    applies: Arc<AtomicU32>,
}

impl Plugin for VersionedDependent {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = ApplyBoom;

    fn inject(&self) -> InjectSpec {
        InjectSpec::none().require(Versioned::NAME)
    }

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), ApplyBoom> {
        self.applies.fetch_add(1, Ordering::SeqCst);
        let version = ctx
            .try_service::<Versioned>()
            .map_err(|error| ApplyBoom(error.to_string()))?
            .0;
        if version == 1 {
            Err(ApplyBoom("reject version 1".to_owned()))
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn ready_reports_typed_current_failure_and_drives_a_new_target() {
    let root = Context::new();
    let applies = Arc::new(AtomicU32::new(0));
    let dependent = root
        .spawn(prepared(VersionedDependent {
            applies: applies.clone(),
        }))
        .await
        .unwrap();
    assert_eq!(dependent.state(), FiberState::Pending);

    let first = root.provide(Arc::new(Versioned(1))).unwrap();
    let failure = dependent
        .ready()
        .await
        .expect_err("version 1 parks the dependent Failed");
    let ReadyError::Apply(failure) = failure else {
        panic!("expected typed apply failure, got {failure:?}");
    };
    assert_eq!(failure.kind(), PluginFailureKind::ReturnedError);
    assert_eq!(failure.diagnostic(), "reject version 1");
    assert_eq!(dependent.state(), FiberState::Failed);
    assert_eq!(applies.load(Ordering::SeqCst), 1);

    let root_for_thread = root.clone();
    let second = std::thread::spawn(move || {
        first.remove().unwrap();
        root_for_thread.provide(Arc::new(Versioned(2))).unwrap()
    })
    .join()
    .unwrap();

    assert_eq!(
        dependent.state(),
        FiberState::Failed,
        "off-runtime drift commits without an executor-dependent state change"
    );
    assert_eq!(
        dependent.ready().await.unwrap(),
        FiberState::Active,
        "ready must drive the latest SemanticTarget instead of returning the stale T1 failure"
    );
    assert_eq!(applies.load(Ordering::SeqCst), 2);
    drop(second);
}

#[tokio::test]
async fn wait_state_is_passive_for_off_runtime_drift() {
    let root = Context::new();
    let applies = Arc::new(AtomicU32::new(0));
    let dependent = root
        .spawn(prepared(VersionedDependent {
            applies: applies.clone(),
        }))
        .await
        .unwrap();
    assert_eq!(dependent.state(), FiberState::Pending);
    assert_eq!(
        dependent.ready().await.unwrap(),
        FiberState::Pending,
        "ready reports stable Pending when the current target is Missing"
    );

    let root_for_thread = root.clone();
    let publication =
        std::thread::spawn(move || root_for_thread.provide(Arc::new(Versioned(2))).unwrap())
            .join()
            .unwrap();

    let waited = dependent
        .wait_state(FiberState::Active, Duration::from_millis(40))
        .await;
    assert!(matches!(waited, Err(WaitStateError::Elapsed)));
    assert_eq!(dependent.state(), FiberState::Pending);
    assert_eq!(
        applies.load(Ordering::SeqCst),
        0,
        "wait_state must never consume or drive the committed recheck"
    );

    assert_eq!(dependent.ready().await.unwrap(), FiberState::Active);
    assert_eq!(applies.load(Ordering::SeqCst), 1);
    drop(publication);
}

struct FailsAfterBlockingCleanup {
    cleanup_started: Arc<tokio::sync::Notify>,
    cleanup_release: Arc<tokio::sync::Notify>,
    captured: Arc<parking_lot::Mutex<Option<Context>>>,
}

impl Plugin for FailsAfterBlockingCleanup {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = ApplyBoom;

    fn inject(&self) -> InjectSpec {
        InjectSpec::none().require(Versioned::NAME)
    }

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), ApplyBoom> {
        *self.captured.lock() = Some(ctx.clone());
        let started = self.cleanup_started.clone();
        let release = self.cleanup_release.clone();
        ctx.effect(move || async move {
            started.notify_one();
            release.notified().await;
        })
        .map_err(|error| ApplyBoom(error.to_string()))?;
        Err(ApplyBoom("rollback probe".to_owned()))
    }
}

#[tokio::test]
async fn failed_is_published_only_after_rollback_finishes() {
    let root = Context::new();
    let cleanup_started = Arc::new(tokio::sync::Notify::new());
    let cleanup_release = Arc::new(tokio::sync::Notify::new());
    let captured = Arc::new(parking_lot::Mutex::new(None));
    let dependent = root
        .spawn(prepared(FailsAfterBlockingCleanup {
            cleanup_started: cleanup_started.clone(),
            cleanup_release: cleanup_release.clone(),
            captured: captured.clone(),
        }))
        .await
        .unwrap();
    assert_eq!(dependent.state(), FiberState::Pending);

    let failed_waiter = tokio::spawn({
        let dependent = dependent.clone();
        async move {
            dependent
                .wait_state(FiberState::Failed, Duration::from_secs(2))
                .await
        }
    });
    let _provider = root.provide(Arc::new(Versioned(1))).unwrap();
    cleanup_started.notified().await;

    assert_eq!(
        dependent.state(),
        FiberState::Unloading,
        "Failed must not publish while rollback cleanup is still in progress"
    );
    assert!(
        !failed_waiter.is_finished(),
        "a Failed waiter must not finish before the rollback barrier"
    );
    let failed_ctx = captured.lock().clone().expect("apply captured its Context");
    assert!(
        matches!(
            failed_ctx.effect_sync(|| {}),
            Err(EffectRegistrationError::InactiveContext)
        ),
        "Unloading must already have a closed generation gate while rollback runs"
    );

    cleanup_release.notify_one();
    failed_waiter.await.unwrap().unwrap();
    assert_eq!(dependent.state(), FiberState::Failed);

    let err = dependent.ready().await.unwrap_err();
    let ReadyError::Apply(failure) = err else {
        panic!("expected typed apply failure, got {err:?}");
    };
    assert_eq!(failure.diagnostic(), "rollback probe");
}

struct PanicsAfterCleanup {
    cleaned: Arc<AtomicU32>,
}

impl Plugin for PanicsAfterCleanup {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = ApplyBoom;

    fn inject(&self) -> InjectSpec {
        InjectSpec::none().require(Versioned::NAME)
    }

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), ApplyBoom> {
        let cleaned = self.cleaned.clone();
        ctx.effect_sync(move || {
            cleaned.fetch_add(1, Ordering::SeqCst);
        })
        .map_err(|error| ApplyBoom(error.to_string()))?;
        panic!("later apply panic");
    }
}

#[tokio::test]
async fn ready_preserves_panicking_apply_kind_after_completed_rollback() {
    let root = Context::new();
    let cleaned = Arc::new(AtomicU32::new(0));
    let dependent = root
        .spawn(prepared(PanicsAfterCleanup {
            cleaned: cleaned.clone(),
        }))
        .await
        .unwrap();
    let _provider = root.provide(Arc::new(Versioned(2))).unwrap();

    let err = dependent.ready().await.unwrap_err();
    let ReadyError::Apply(failure) = err else {
        panic!("expected typed apply failure, got {err:?}");
    };
    assert_eq!(failure.kind(), PluginFailureKind::Panic);
    assert_eq!(failure.diagnostic(), "later apply panic");
    assert_eq!(dependent.state(), FiberState::Failed);
    assert_eq!(
        cleaned.load(Ordering::SeqCst),
        1,
        "Failed and ReadyError::Apply are published only after panic rollback"
    );
}

struct BlocksSuccessfulApply {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

impl Plugin for BlocksSuccessfulApply {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = ApplyBoom;

    fn inject(&self) -> InjectSpec {
        InjectSpec::none().require(Versioned::NAME)
    }

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), ApplyBoom> {
        ctx.effect_sync(|| {})
            .map_err(|error| ApplyBoom(error.to_string()))?;
        self.entered.notify_one();
        self.release.notified().await;
        Ok(())
    }
}

#[tokio::test]
async fn loading_and_active_publish_at_their_apply_barriers() {
    let root = Context::new();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let dependent = root
        .spawn(prepared(BlocksSuccessfulApply {
            entered: entered.clone(),
            release: release.clone(),
        }))
        .await
        .unwrap();
    assert_eq!(dependent.state(), FiberState::Pending);

    let active_waiter = tokio::spawn({
        let dependent = dependent.clone();
        async move {
            dependent
                .wait_state(FiberState::Active, Duration::from_secs(2))
                .await
        }
    });
    let _provider = root.provide(Arc::new(Versioned(2))).unwrap();
    entered.notified().await;

    assert_eq!(dependent.state(), FiberState::Loading);
    dependent
        .wait_state(FiberState::Loading, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(
        !active_waiter.is_finished(),
        "Active must not publish before the successful apply returns"
    );

    release.notify_one();
    active_waiter.await.unwrap().unwrap();
    assert_eq!(dependent.state(), FiberState::Active);
}

struct BlocksTerminalCleanup {
    cleanup_started: Arc<tokio::sync::Notify>,
    cleanup_release: Arc<tokio::sync::Notify>,
}

impl Plugin for BlocksTerminalCleanup {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = ApplyBoom;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), ApplyBoom> {
        let started = self.cleanup_started.clone();
        let release = self.cleanup_release.clone();
        ctx.effect(move || async move {
            started.notify_one();
            release.notified().await;
        })
        .map_err(|error| ApplyBoom(error.to_string()))?;
        Ok(())
    }
}

#[tokio::test]
async fn disposed_is_published_only_after_terminal_cleanup_finishes() {
    let root = Context::new();
    let cleanup_started = Arc::new(tokio::sync::Notify::new());
    let cleanup_release = Arc::new(tokio::sync::Notify::new());
    let fiber_handle = root
        .spawn(prepared(BlocksTerminalCleanup {
            cleanup_started: cleanup_started.clone(),
            cleanup_release: cleanup_release.clone(),
        }))
        .await
        .unwrap();
    assert_eq!(fiber_handle.state(), FiberState::Active);

    let disposed_waiter = tokio::spawn({
        let fiber_handle = fiber_handle.clone();
        async move {
            fiber_handle
                .wait_state(FiberState::Disposed, Duration::from_secs(2))
                .await
        }
    });
    let disposer = tokio::spawn({
        let fiber_handle = fiber_handle.clone();
        async move { fiber_handle.dispose().await }
    });
    cleanup_started.notified().await;

    assert_eq!(fiber_handle.state(), FiberState::Unloading);
    assert!(
        !disposed_waiter.is_finished(),
        "Disposed must not publish before terminal generation cleanup completes"
    );

    cleanup_release.notify_one();
    disposer.await.unwrap().unwrap();
    disposed_waiter.await.unwrap().unwrap();
    assert_eq!(fiber_handle.state(), FiberState::Disposed);
    assert_eq!(
        fiber_handle.ready().await.unwrap(),
        FiberState::Disposed,
        "ready reports the terminal closed outcome instead of a transient state"
    );
}
