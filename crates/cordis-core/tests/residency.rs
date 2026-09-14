//! LF-11 public evidence: Registry residency is independent of FiberHandles and
//! spawn origin, while lifecycle control remains explicit.

mod common;

use common::ordinary_fiber_count;

use cordis_core::lifecycle::SpawnError;
use cordis_core::{Context, FiberHandle, FiberState, Plugin, PreparedPlugin, Service};
use parking_lot::Mutex;
use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Marker;

impl Service for Marker {
    const NAME: &'static str = "ticket-23-marker";
}

struct Resident {
    disposed: Arc<AtomicUsize>,
}

impl Plugin for Resident {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn apply(
        &self,
        ctx: Context,
        _prepared: &(),
    ) -> impl Future<Output = Result<(), Infallible>> + Send {
        let disposed = self.disposed.clone();
        async move {
            ctx.effect_sync(move || {
                disposed.fetch_add(1, Ordering::SeqCst);
            })
            .expect("an applying Fiber admits cleanup");
            let _ = ctx
                .provide::<Marker>(Arc::new(Marker))
                .expect("the marker slot is vacant");
            Ok(())
        }
    }
}

struct Origin {
    child: Arc<Mutex<Option<FiberHandle>>>,
    child_disposed: Arc<AtomicUsize>,
}

struct CaptureOrigin(Arc<Mutex<Option<Context>>>);

impl Plugin for CaptureOrigin {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn apply(
        &self,
        ctx: Context,
        _prepared: &(),
    ) -> impl Future<Output = Result<(), Infallible>> + Send {
        *self.0.lock() = Some(ctx);
        std::future::ready(Ok(()))
    }
}

struct BlockingResident {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    disposed: Arc<AtomicUsize>,
}

impl Plugin for BlockingResident {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn apply(
        &self,
        ctx: Context,
        _prepared: &(),
    ) -> impl Future<Output = Result<(), Infallible>> + Send {
        let entered = self.entered.clone();
        let release = self.release.clone();
        let disposed = self.disposed.clone();
        async move {
            ctx.effect_sync(move || {
                disposed.fetch_add(1, Ordering::SeqCst);
            })
            .expect("an admitted child owns its cleanup");
            entered.notify_one();
            release.notified().await;
            Ok(())
        }
    }
}

impl Plugin for Origin {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = SpawnError;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn apply(
        &self,
        ctx: Context,
        _prepared: &(),
    ) -> impl Future<Output = Result<(), SpawnError>> + Send {
        let child = self.child.clone();
        let child_disposed = self.child_disposed.clone();
        async move {
            let fiber_handle = ctx
                .spawn(PreparedPlugin::from_input(
                    Resident {
                        disposed: child_disposed,
                    },
                    (),
                ))
                .await?;
            *child.lock() = Some(fiber_handle);
            Ok(())
        }
    }
}

#[tokio::test]
async fn admitted_child_outlives_disposed_spawn_origin() {
    let runtime = Context::new();
    let child = Arc::new(Mutex::new(None));
    let child_disposed = Arc::new(AtomicUsize::new(0));
    let origin = runtime
        .spawn(PreparedPlugin::from_input(
            Origin {
                child: child.clone(),
                child_disposed: child_disposed.clone(),
            },
            (),
        ))
        .await
        .unwrap();

    origin.dispose().await.unwrap();

    let child = child.lock().take().unwrap();
    assert_eq!(child.state(), FiberState::Active);
    assert_eq!(child_disposed.load(Ordering::SeqCst), 0);
    assert_eq!(ordinary_fiber_count(&runtime), 1);

    child.dispose().await.unwrap();
    assert_eq!(child_disposed.load(Ordering::SeqCst), 1);
    assert_eq!(ordinary_fiber_count(&runtime), 0);
}

#[tokio::test]
async fn admission_first_child_finishes_after_its_origin_is_disposed() {
    let runtime = Context::new();
    let captured = Arc::new(Mutex::new(None));
    let origin = runtime
        .spawn(PreparedPlugin::from_input(
            CaptureOrigin(captured.clone()),
            (),
        ))
        .await
        .unwrap();
    let origin_context = captured.lock().take().unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let child_disposed = Arc::new(AtomicUsize::new(0));

    let child_spawn = tokio::spawn({
        let entered = entered.clone();
        let release = release.clone();
        let child_disposed = child_disposed.clone();
        async move {
            origin_context
                .spawn(PreparedPlugin::from_input(
                    BlockingResident {
                        entered,
                        release,
                        disposed: child_disposed,
                    },
                    (),
                ))
                .await
        }
    });
    entered.notified().await;

    origin.dispose().await.unwrap();
    assert_eq!(ordinary_fiber_count(&runtime), 1);
    release.notify_one();

    let child = child_spawn.await.unwrap().unwrap();
    assert_eq!(child.state(), FiberState::Active);
    assert_eq!(child_disposed.load(Ordering::SeqCst), 0);
    child.dispose().await.unwrap();
}

#[tokio::test]
async fn dropping_every_fiber_handle_does_not_end_a_resident_fiber() {
    let runtime = Context::new();
    let disposed = Arc::new(AtomicUsize::new(0));
    let fiber_handle = runtime
        .spawn(PreparedPlugin::from_input(
            Resident {
                disposed: disposed.clone(),
            },
            (),
        ))
        .await
        .unwrap();

    drop(fiber_handle);

    assert_eq!(ordinary_fiber_count(&runtime), 1);
    assert!(runtime.try_service::<Marker>().is_ok());
    assert_eq!(disposed.load(Ordering::SeqCst), 0);
}
