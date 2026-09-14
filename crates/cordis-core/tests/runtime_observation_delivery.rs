//! Issue 43 evidence for detached protocol-first Runtime observation delivery.

mod common;

use cordis_core::event::{Event, observer, observer_sync};
use cordis_core::observation::RuntimeObservation;
use cordis_core::{Context, Plugin, PreparedPlugin, Routing};
use parking_lot::Mutex;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn prepared<P>(plugin: P) -> PreparedPlugin
where
    P: Plugin<Config = (), Input = (), PrepareError = Infallible>,
{
    PreparedPlugin::from_input(plugin, ())
}

struct Plain;
impl Plugin for Plain {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, _ctx: Context, _: &()) -> Result<(), Infallible> {
        Ok(())
    }
}

struct Registering {
    hits: Arc<AtomicUsize>,
}
impl Plugin for Registering {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, ctx: Context, _: &()) -> Result<(), Infallible> {
        let hits = self.hits.clone();
        ctx.observe_runtime(observer(
            move |_registration: Context, record: RuntimeObservation| {
                let hits = hits.clone();
                async move {
                    if matches!(
                        record,
                        RuntimeObservation::DispatchCompleted {
                            event: Tick::NAME,
                            ..
                        }
                    ) {
                        hits.fetch_add(1, Ordering::SeqCst);
                    }
                    Ok::<_, Infallible>(())
                }
            },
        ))
        .unwrap();
        Ok(())
    }
}

struct Tick;
impl Event for Tick {
    const NAME: &'static str = "issue43-tick";
    type Args = ();
    type Output = ();
}

#[test]
fn missing_executor_drops_delivery_without_changing_listener_truth() {
    let ctx = Context::new();
    let hits = Arc::new(AtomicUsize::new(0));
    let observed = hits.clone();
    ctx.observe_runtime(observer(move |_: Context, _: RuntimeObservation| {
        let observed = observed.clone();
        async move {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok::<_, Infallible>(())
        }
    }))
    .unwrap();
    let registration = ctx
        .on::<Tick, _>(observer_sync(|_: Context, _: ()| Ok::<_, Infallible>(())))
        .unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    assert!(registration.remove());
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn blocked_observers_run_in_parallel_without_delaying_source_and_keep_attribution() {
    let ctx = Context::new();
    let release = Arc::new(tokio::sync::Notify::new());
    let active = Arc::new(AtomicUsize::new(0));
    let maximum = Arc::new(AtomicUsize::new(0));
    let attributed = Arc::new(Mutex::new(None));
    let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();

    let r = release.clone();
    let a = active.clone();
    let m = maximum.clone();
    let attribution = attributed.clone();
    let entered = entered_tx.clone();
    ctx.observe_runtime(observer(
        move |registration: Context, record: RuntimeObservation| {
            let r = r.clone();
            let a = a.clone();
            let m = m.clone();
            let attribution = attribution.clone();
            let entered = entered.clone();
            async move {
                if matches!(
                    record,
                    RuntimeObservation::DispatchCompleted {
                        event: Tick::NAME,
                        ..
                    }
                ) {
                    *attribution.lock() = Some(registration.runtime_snapshot().fibers().len());
                    let current = a.fetch_add(1, Ordering::SeqCst) + 1;
                    m.fetch_max(current, Ordering::SeqCst);
                    entered
                        .send(())
                        .expect("blocked-observer sentinel is alive");
                    r.notified().await;
                    a.fetch_sub(1, Ordering::SeqCst);
                }
                Ok::<_, Infallible>(())
            }
        },
    ))
    .unwrap();

    let r = release.clone();
    let a = active.clone();
    let m = maximum.clone();
    let entered = entered_tx.clone();
    ctx.observe_runtime(observer(move |_: Context, record: RuntimeObservation| {
        let r = r.clone();
        let a = a.clone();
        let m = m.clone();
        let entered = entered.clone();
        async move {
            if matches!(
                record,
                RuntimeObservation::DispatchCompleted {
                    event: Tick::NAME,
                    ..
                }
            ) {
                let current = a.fetch_add(1, Ordering::SeqCst) + 1;
                m.fetch_max(current, Ordering::SeqCst);
                entered
                    .send(())
                    .expect("blocked-observer sentinel is alive");
                r.notified().await;
                a.fetch_sub(1, Ordering::SeqCst);
            }
            Ok::<_, Infallible>(())
        }
    }))
    .unwrap();

    let source_ctx = ctx.clone();
    let source = tokio::spawn(async move {
        source_ctx
            .emit::<Tick>(Routing::Unscoped, ())
            .await
            .unwrap();
    });
    drop(entered_tx);
    common::bounded(5_000, async {
        entered_rx.recv().await.expect("first observer entered");
        entered_rx.recv().await.expect("second observer entered");
    })
    .await
    .expect("both blocked observers must enter deterministically");
    common::bounded(5_000, source)
        .await
        .expect("source completion must not await observer work")
        .unwrap();
    ctx.observe_runtime(observer_sync(|_, _| Ok::<_, Infallible>(())))
        .expect("observer bookkeeping remains available while callbacks are blocked");
    assert_eq!(
        maximum.load(Ordering::SeqCst),
        2,
        "both observer tasks must enter before either blocker is released"
    );
    assert!(attributed.lock().is_some());
    release.notify_waiters();
}

#[tokio::test]
async fn failing_and_panicking_observers_do_not_veto_and_later_observers_are_attempted() {
    let ctx = Context::new();
    let hits = Arc::new(AtomicUsize::new(0));
    ctx.observe_runtime(observer(|_: Context, _: RuntimeObservation| async {
        Err::<(), _>(std::io::Error::other("observer failure"))
    }))
    .unwrap();
    ctx.observe_runtime(observer(|_: Context, _: RuntimeObservation| async {
        panic!("observer panic");
        #[allow(unreachable_code)]
        Ok::<(), Infallible>(())
    }))
    .unwrap();
    let h = hits.clone();
    ctx.observe_runtime(observer(move |_: Context, _: RuntimeObservation| {
        let h = h.clone();
        async move {
            h.fetch_add(1, Ordering::SeqCst);
            Ok::<_, Infallible>(())
        }
    }))
    .unwrap();
    let fiber_handle = ctx.spawn(prepared(Plain)).await.unwrap();
    for _ in 0..100 {
        if hits.load(Ordering::SeqCst) > 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(hits.load(Ordering::SeqCst) > 0);
    fiber_handle.dispose().await.unwrap();
}

#[tokio::test]
async fn registering_generation_close_stops_future_delivery() {
    let ctx = Context::new();
    let hits = Arc::new(AtomicUsize::new(0));
    let owner = ctx
        .with_child_scope()
        .spawn(prepared(Registering { hits: hits.clone() }))
        .await
        .unwrap();
    ctx.emit::<Tick>(Routing::Unscoped, ()).await.unwrap();
    for _ in 0..100 {
        if hits.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "child-scope subscription is Runtime-wide"
    );
    owner.dispose().await.unwrap();
    ctx.emit::<Tick>(Routing::Unscoped, ()).await.unwrap();
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

struct ReentrantObserverCapture {
    root: Context,
    reentered: Arc<AtomicUsize>,
}

impl Drop for ReentrantObserverCapture {
    fn drop(&mut self) {
        self.root
            .observe_runtime(observer_sync(|_, _| Ok::<_, Infallible>(())))
            .expect("root remains open while an observer capture is destroyed");
        self.reentered.fetch_add(1, Ordering::SeqCst);
    }
}

struct DropObserverCapture {
    root: Context,
    reentered: Arc<AtomicUsize>,
}

impl Plugin for DropObserverCapture {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _: &()) -> Result<(), Infallible> {
        let capture = ReentrantObserverCapture {
            root: self.root.clone(),
            reentered: self.reentered.clone(),
        };
        ctx.observe_runtime(observer_sync(move |_, _| {
            let _keep_capture_alive = &capture;
            Ok::<_, Infallible>(())
        }))
        .unwrap();
        Ok(())
    }
}

#[tokio::test]
async fn observer_capture_destruction_can_reenter_observer_registration() {
    let root = Context::new();
    let reentered = Arc::new(AtomicUsize::new(0));
    let fiber_handle = root
        .spawn(prepared(DropObserverCapture {
            root: root.clone(),
            reentered: reentered.clone(),
        }))
        .await
        .unwrap();

    common::bounded(5_000, fiber_handle.dispose())
        .await
        .expect("observer capture destruction deadlocked")
        .unwrap();
    assert_eq!(reentered.load(Ordering::SeqCst), 1);
}
