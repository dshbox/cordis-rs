//! A user Input destructor can mutate a Service after old-era disposal.
use cordis_core::{
    Context, FiberState, InjectSpec, Plugin, PreparedChange, PreparedPlugin, Service,
};
use parking_lot::Mutex;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::Notify;

struct Value;
impl Service for Value {
    const NAME: &'static str = "era-input-drop-terminal";
}
type PublicationCell = Arc<Mutex<Option<cordis_core::service::ServicePublication<Value>>>>;
struct Input {
    on_drop: Option<(Context, PublicationCell)>,
}
impl Drop for Input {
    fn drop(&mut self) {
        if let Some((root, publication)) = self.on_drop.take() {
            *publication.lock() = Some(root.provide(Arc::new(Value)).unwrap());
            panic!("old era input destructor");
        }
    }
}
struct Source;
impl Plugin for Source {
    type Config = ();
    type Input = Input;
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, _: ()) -> Result<Input, Infallible> {
        Ok(Input { on_drop: None })
    }
    async fn apply(&self, ctx: Context, _: &Input) -> Result<(), Infallible> {
        let _ = ctx.provide(Arc::new(Value)).unwrap();
        Ok(())
    }
}
struct Dependent {
    entered: Arc<Notify>,
    release: Arc<Notify>,
    applies: Arc<std::sync::atomic::AtomicUsize>,
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
        if self
            .applies
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            == 1
        {
            self.entered.notify_one();
            self.release.notified().await;
        }
        Ok(())
    }
}
#[tokio::test]
async fn input_drop_panic_waits_for_final_dependent_convergence() {
    let root = Context::new();
    let publication = Arc::new(Mutex::new(None));
    let source = root
        .spawn(PreparedPlugin::from_input(
            Source,
            Input {
                on_drop: Some((root.clone(), publication.clone())),
            },
        ))
        .await
        .unwrap();
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let applies = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let dependent = root
        .spawn(PreparedPlugin::from_input(
            Dependent {
                entered: entered.clone(),
                release: release.clone(),
                applies: applies.clone(),
            },
            (),
        ))
        .await
        .unwrap();
    assert_eq!(dependent.state(), FiberState::Active);

    let swap = tokio::spawn({
        let source = source.clone();
        async move {
            source
                .era_swap(PreparedChange::from_input::<Source>(Input {
                    on_drop: None,
                }))
                .await
        }
    });
    entered.notified().await;
    assert_eq!(source.state(), FiberState::Disposed);
    assert_eq!(dependent.state(), FiberState::Loading);
    let observed_dependent = dependent.clone();
    let (polled, observed_poll) = tokio::sync::oneshot::channel();
    let observer = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut ready = Box::pin(observed_dependent.ready());
            assert!(futures::poll!(&mut ready).is_pending());
            polled.send(()).unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(3), ready)
                .await
                .expect("cross-runtime ready must terminate")
        })
    });
    observed_poll.await.unwrap();
    assert!(
        !swap.is_finished(),
        "the caller must await the final dependent barrier even on Drop panic"
    );

    release.notify_one();
    let error = tokio::time::timeout(std::time::Duration::from_secs(3), swap)
        .await
        .expect("a committed era must terminate")
        .expect_err("old Input Drop must retain its panic");
    let payload = error.into_panic();
    assert_eq!(
        payload.downcast_ref::<&str>(),
        Some(&"old era input destructor")
    );
    assert_eq!(observer.join().unwrap().unwrap(), FiberState::Active);
    assert_eq!(dependent.ready().await.unwrap(), FiberState::Active);
    assert_eq!(root.runtime_snapshot().fibers().len(), 2);
}

struct PanicReport(Mutex<Option<tokio::sync::oneshot::Sender<String>>>);
impl cordis_core::logger::Exporter for PanicReport {
    fn export(&self, record: &cordis_core::logger::LogRecord) {
        if let Some(send) = self.0.lock().take() {
            let _ = send.send(record.text().to_owned());
        }
    }
}

#[tokio::test]
async fn canceled_caller_still_reports_original_drop_panic_after_final_barrier() {
    let root = Context::new();
    let (send, mut receive) = tokio::sync::oneshot::channel();
    let _registration = root
        .add_exporter(Arc::new(PanicReport(Mutex::new(Some(send)))))
        .unwrap();
    let publication = Arc::new(Mutex::new(None));
    let source = root
        .spawn(PreparedPlugin::from_input(
            Source,
            Input {
                on_drop: Some((root.clone(), publication)),
            },
        ))
        .await
        .unwrap();
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let dependent = root
        .spawn(PreparedPlugin::from_input(
            Dependent {
                entered: entered.clone(),
                release: release.clone(),
                applies: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            },
            (),
        ))
        .await
        .unwrap();
    let swap = tokio::spawn({
        let source = source.clone();
        async move {
            source
                .era_swap(PreparedChange::from_input::<Source>(Input {
                    on_drop: None,
                }))
                .await
        }
    });
    entered.notified().await;
    swap.abort();
    assert!(swap.await.unwrap_err().is_cancelled());
    assert_eq!(source.ready().await.unwrap(), FiberState::Disposed);
    assert!(matches!(
        receive.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    release.notify_one();
    let record = tokio::time::timeout(std::time::Duration::from_secs(3), receive)
        .await
        .expect("owner must finish and report the panic")
        .unwrap();
    assert!(record.contains("old era input destructor"), "{record}");
    assert_eq!(dependent.ready().await.unwrap(), FiberState::Active);
}
