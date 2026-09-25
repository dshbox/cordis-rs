//! Public-path regression for Service-dependent convergence ownership.

use cordis_core::lifecycle::ReadyError;
use cordis_core::logger::{Exporter, ExporterRegistration, LogRecord};
use cordis_core::{Context, FiberState, InjectSpec, Plugin, PreparedPlugin, Service};
use parking_lot::Mutex;
use std::convert::Infallible;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Notify;

struct Value;
impl Service for Value {
    const NAME: &'static str = "service-convergence-owner";
}

struct PanicPayload {
    dropped: Arc<Notify>,
}

impl Drop for PanicPayload {
    fn drop(&mut self) {
        self.dropped.notify_one();
        panic!("panic payload destructor");
    }
}

struct Dependent {
    applies: Arc<AtomicUsize>,
    payload_dropped: Arc<Notify>,
}

impl Plugin for Dependent {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn inject(&self) -> InjectSpec {
        InjectSpec::none().require(Value::NAME)
    }

    fn prepare(&self, _: ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, _: Context, _: &()) -> Result<(), Infallible> {
        if self.applies.fetch_add(1, Ordering::SeqCst) == 1 {
            std::panic::panic_any(PanicPayload {
                dropped: self.payload_dropped.clone(),
            });
        }
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn payload_drop_panic_cannot_orphan_service_convergence_owner() {
    let root = Context::new();
    let first = root.provide(Arc::new(Value)).unwrap();
    let applies = Arc::new(AtomicUsize::new(0));
    let payload_dropped = Arc::new(Notify::new());
    let dependent = root
        .spawn(PreparedPlugin::from_input(
            Dependent {
                applies: applies.clone(),
                payload_dropped: payload_dropped.clone(),
            },
            (),
        ))
        .await
        .unwrap();
    assert_eq!(dependent.state(), FiberState::Active);
    assert_eq!(applies.load(Ordering::SeqCst), 1);

    first.remove().unwrap();
    assert_eq!(dependent.ready().await.unwrap(), FiberState::Pending);

    let payload_drop = payload_dropped.notified();
    tokio::pin!(payload_drop);
    payload_drop.as_mut().enable();

    let _second = root.provide(Arc::new(Value)).unwrap();

    tokio::time::timeout(Duration::from_secs(3), &mut payload_drop)
        .await
        .expect("the caught Plugin panic payload must be destroyed");

    let observed = dependent.clone();
    let waiter = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(3), observed.ready()).await
        })
    });

    let result = waiter
        .join()
        .unwrap()
        .expect("cross-runtime ready must observe terminal convergence");
    let failure = match result {
        Err(ReadyError::Apply(failure)) => failure,
        other => panic!("expected the contained apply failure, got {other:?}"),
    };
    assert!(
        failure
            .diagnostic()
            .contains("panic payload destructor panicked: panic payload destructor"),
        "{}",
        failure.diagnostic()
    );
    assert_eq!(dependent.state(), FiberState::Failed);
    assert_eq!(applies.load(Ordering::SeqCst), 2);
}

#[derive(Debug)]
struct CleanupFailure;

impl fmt::Display for CleanupFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("cleanup failed")
    }
}

impl std::error::Error for CleanupFailure {}

struct CleanupDependent;

impl Plugin for CleanupDependent {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn inject(&self) -> InjectSpec {
        InjectSpec::none().require(Value::NAME)
    }

    fn prepare(&self, _: ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _: &()) -> Result<(), Infallible> {
        ctx.effect_sync(|| -> Result<(), CleanupFailure> { Err(CleanupFailure) })
            .unwrap();
        Ok(())
    }
}

struct ExporterPanicPayload {
    dropped: Arc<Notify>,
}

impl Drop for ExporterPanicPayload {
    fn drop(&mut self) {
        self.dropped.notify_one();
        panic!("exporter panic payload destructor");
    }
}

struct CleanupFailureExporter {
    payload_dropped: Arc<Notify>,
}

impl Exporter for CleanupFailureExporter {
    fn export(&self, record: &LogRecord) {
        if record.text().contains("effect cleanup") {
            std::panic::panic_any(ExporterPanicPayload {
                dropped: self.payload_dropped.clone(),
            });
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exporter_payload_drop_panic_cannot_orphan_cleanup_convergence_owner() {
    let root = Context::new();
    let first = root.provide(Arc::new(Value)).unwrap();
    let payload_dropped = Arc::new(Notify::new());
    let _exporter = root
        .add_exporter(Arc::new(CleanupFailureExporter {
            payload_dropped: payload_dropped.clone(),
        }))
        .unwrap();
    let dependent = root
        .spawn(PreparedPlugin::from_input(CleanupDependent, ()))
        .await
        .unwrap();
    assert_eq!(dependent.state(), FiberState::Active);

    let payload_drop = payload_dropped.notified();
    tokio::pin!(payload_drop);
    payload_drop.as_mut().enable();

    first.remove().unwrap();

    tokio::time::timeout(Duration::from_secs(3), &mut payload_drop)
        .await
        .expect("the caught exporter panic payload must be destroyed");

    let observed = dependent.clone();
    let waiter = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(3), observed.ready()).await
        })
    });

    let result = waiter
        .join()
        .unwrap()
        .expect("cross-runtime ready must observe terminal convergence");
    assert_eq!(result.unwrap(), FiberState::Pending);
    assert_eq!(dependent.state(), FiberState::Pending);
}

struct SelfRemovingExporter {
    registration: Arc<Mutex<Option<ExporterRegistration>>>,
    dropped: Arc<Notify>,
}

impl Exporter for SelfRemovingExporter {
    fn export(&self, record: &LogRecord) {
        if record.text().contains("effect cleanup") {
            let registration = self.registration.lock().take().unwrap();
            assert!(registration.remove());
        }
    }
}

impl Drop for SelfRemovingExporter {
    fn drop(&mut self) {
        self.dropped.notify_one();
        panic!("exporter object Drop panicked");
    }
}

struct FollowingExporter(Arc<AtomicUsize>);

impl Exporter for FollowingExporter {
    fn export(&self, record: &LogRecord) {
        if record.text().contains("effect cleanup") {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exporter_object_drop_after_self_removal_cannot_orphan_convergence() {
    let root = Context::new();
    let first = root.provide(Arc::new(Value)).unwrap();
    let registration = Arc::new(Mutex::new(None));
    let dropped = Arc::new(Notify::new());
    let self_removing = root
        .add_exporter(Arc::new(SelfRemovingExporter {
            registration: registration.clone(),
            dropped: dropped.clone(),
        }))
        .unwrap();
    *registration.lock() = Some(self_removing);

    let following_calls = Arc::new(AtomicUsize::new(0));
    let _following = root
        .add_exporter(Arc::new(FollowingExporter(following_calls.clone())))
        .unwrap();
    let dependent = root
        .spawn(PreparedPlugin::from_input(CleanupDependent, ()))
        .await
        .unwrap();
    assert_eq!(dependent.state(), FiberState::Active);

    let exporter_drop = dropped.notified();
    tokio::pin!(exporter_drop);
    exporter_drop.as_mut().enable();
    first.remove().unwrap();
    tokio::time::timeout(Duration::from_secs(3), &mut exporter_drop)
        .await
        .expect("self-removal must drop the last exporter snapshot reference");

    let observed = dependent.clone();
    let waiter = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(3), observed.ready()).await
        })
    });
    let state = waiter
        .join()
        .unwrap()
        .expect("cross-runtime ready must observe the completed convergence")
        .unwrap();
    assert_eq!(state, FiberState::Pending);
    assert_eq!(following_calls.load(Ordering::SeqCst), 1);
}
