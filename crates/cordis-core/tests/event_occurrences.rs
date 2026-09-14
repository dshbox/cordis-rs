//! Issue 32 contract: exact listener occurrences, removal claims, and once consumption.

mod common;

use cordis_core::event::{
    DispatchError, InvocationFailureKind, ListenerOptions, ListenerRegistration,
    ListenerRegistrationError, around, mapper_sync, observer, observer_sync, responder_sync,
};
use cordis_core::{Context, Event, Plugin, PreparedPlugin, QueryOutcome, Routing};
use parking_lot::Mutex;
use std::borrow::Cow;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Ping;
impl Event for Ping {
    const NAME: &'static str = "issue32/ping";
    type Args = usize;
    type Output = usize;
}

struct Flow;
impl Event for Flow {
    const NAME: &'static str = "issue32/flow";
    type Args = usize;
    type Output = usize;
}

#[tokio::test]
async fn duplicate_occurrences_are_independent_and_registration_drop_is_inert() {
    let ctx = Context::new();
    static HITS: AtomicUsize = AtomicUsize::new(0);
    HITS.store(0, Ordering::SeqCst);
    fn callback(_: Context, _: usize) -> Result<(), Infallible> {
        HITS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    let first = ctx.on::<Ping, _>(observer_sync(callback)).unwrap();
    let second = ctx.on::<Ping, _>(observer_sync(callback)).unwrap();
    drop(first);

    ctx.emit::<Ping>(Routing::Unscoped, 1).await.unwrap();
    assert_eq!(HITS.load(Ordering::SeqCst), 2, "Drop must not unregister");

    assert!(
        second.remove(),
        "the second exact occurrence is still removable"
    );
    ctx.emit::<Ping>(Routing::Unscoped, 2).await.unwrap();
    assert_eq!(
        HITS.load(Ordering::SeqCst),
        3,
        "removing one duplicate leaves the other"
    );
}

#[tokio::test]
async fn remove_before_snapshot_claim_skips_but_claim_before_remove_finishes_owned_work() {
    let ctx = Context::new();
    let (block_started_tx, block_started_rx) = tokio::sync::oneshot::channel();
    let block_started = Arc::new(Mutex::new(Some(block_started_tx)));
    let block_release = Arc::new(tokio::sync::Notify::new());
    let started = block_started.clone();
    let release = block_release.clone();
    ctx.on::<Ping, _>(observer(move |_, _| {
        let started = started.clone();
        let release = release.clone();
        async move {
            if let Some(tx) = started.lock().take() {
                let _ = tx.send(());
            }
            release.notified().await;
            Ok::<(), Infallible>(())
        }
    }))
    .unwrap();

    let skipped_hits = Arc::new(AtomicUsize::new(0));
    let skipped = skipped_hits.clone();
    let victim = ctx
        .on::<Ping, _>(observer_sync(move |_, _| {
            skipped.fetch_add(1, Ordering::SeqCst);
            Ok::<(), Infallible>(())
        }))
        .unwrap();

    let emitter = ctx.clone();
    let dispatch = tokio::spawn(async move { emitter.emit::<Ping>(Routing::Unscoped, 1).await });
    block_started_rx.await.unwrap();
    assert!(victim.remove());
    block_release.notify_one();
    dispatch.await.unwrap().unwrap();
    assert_eq!(skipped_hits.load(Ordering::SeqCst), 0);

    let ctx = Context::new();
    let (owned_started_tx, owned_started_rx) = tokio::sync::oneshot::channel();
    let owned_started = Arc::new(Mutex::new(Some(owned_started_tx)));
    let owned_release = Arc::new(tokio::sync::Notify::new());
    let owned_hits = Arc::new(AtomicUsize::new(0));
    let started = owned_started.clone();
    let release = owned_release.clone();
    let hits = owned_hits.clone();
    let registration = ctx
        .on::<Ping, _>(observer(move |_, _| {
            let started = started.clone();
            let release = release.clone();
            let hits = hits.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                if let Some(tx) = started.lock().take() {
                    let _ = tx.send(());
                }
                release.notified().await;
                Ok::<(), Infallible>(())
            }
        }))
        .unwrap();

    let emitter = ctx.clone();
    let dispatch = tokio::spawn(async move { emitter.emit::<Ping>(Routing::Unscoped, 2).await });
    owned_started_rx.await.unwrap();
    assert!(
        registration.remove(),
        "removal unregisters future claims without joining owned work"
    );
    owned_release.notify_one();
    dispatch.await.unwrap().unwrap();
    assert_eq!(owned_hits.load(Ordering::SeqCst), 1);
    ctx.emit::<Ping>(Routing::Unscoped, 3).await.unwrap();
    assert_eq!(owned_hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn once_is_consumed_before_callback_and_panic_never_restores_it() {
    let ctx = Context::new();
    let hits = Arc::new(AtomicUsize::new(0));
    let callback_hits = hits.clone();
    let registration = ctx
        .on_with::<Ping, _>(
            observer_sync(move |_, _| -> Result<(), Infallible> {
                callback_hits.fetch_add(1, Ordering::SeqCst);
                panic!("once panic")
            }),
            ListenerOptions::default().once(),
        )
        .unwrap();

    let first = ctx.emit::<Ping>(Routing::Unscoped, 1).await.unwrap_err();
    assert!(matches!(
        first,
        DispatchError::Invocation(ref failure) if failure.kind() == InvocationFailureKind::Panic
    ));
    ctx.emit::<Ping>(Routing::Unscoped, 2).await.unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert!(
        !registration.remove(),
        "the winning once claim already unregistered the occurrence"
    );
}

#[tokio::test]
async fn query_short_circuit_and_waterfall_veto_do_not_consume_unreached_once() {
    let query = Context::new();
    let first = query
        .on::<Ping, _>(responder_sync(|_, value| {
            Ok::<_, Infallible>(Some(value + 10))
        }))
        .unwrap();
    let once_hits = Arc::new(AtomicUsize::new(0));
    let hits = once_hits.clone();
    let once = query
        .on_with::<Ping, _>(
            responder_sync(move |_, value| {
                hits.fetch_add(1, Ordering::SeqCst);
                Ok::<_, Infallible>(Some(value + 20))
            }),
            ListenerOptions::default().once(),
        )
        .unwrap();

    assert!(matches!(
        query.query::<Ping>(Routing::Unscoped, 1).await.unwrap(),
        QueryOutcome::Answer(11)
    ));
    assert_eq!(once_hits.load(Ordering::SeqCst), 0);
    assert!(first.remove());
    assert!(matches!(
        query.query::<Ping>(Routing::Unscoped, 2).await.unwrap(),
        QueryOutcome::Answer(22)
    ));
    assert_eq!(once_hits.load(Ordering::SeqCst), 1);
    assert!(!once.remove());

    let waterfall = Context::new();
    let veto = waterfall
        .on::<Flow, _>(around(|_, value, _next| async move {
            Ok::<usize, Infallible>(value + 100)
        }))
        .unwrap();
    let mapper_hits = Arc::new(AtomicUsize::new(0));
    let hits = mapper_hits.clone();
    let mapper = waterfall
        .on_with::<Flow, _>(
            mapper_sync(move |_, value| {
                hits.fetch_add(1, Ordering::SeqCst);
                Ok::<usize, Infallible>(value + 1)
            }),
            ListenerOptions::default().once(),
        )
        .unwrap();

    let out = waterfall
        .waterfall::<Flow, _, _, _>(Routing::Unscoped, 1, |value| async move {
            Ok::<usize, Infallible>(value)
        })
        .await
        .unwrap();
    assert_eq!(out, 101);
    assert_eq!(mapper_hits.load(Ordering::SeqCst), 0);
    assert!(veto.remove());

    let out = waterfall
        .waterfall::<Flow, _, _, _>(Routing::Unscoped, 1, |value| async move {
            Ok::<usize, Infallible>(value)
        })
        .await
        .unwrap();
    assert_eq!(out, 2);
    assert_eq!(mapper_hits.load(Ordering::SeqCst), 1);
    assert!(!mapper.remove());
}

#[tokio::test]
async fn scope_skip_does_not_consume_once() {
    let root = Context::new();
    let child = root.with_child_scope();
    let sibling = root.with_child_scope();
    let hits = Arc::new(AtomicUsize::new(0));
    let callback_hits = hits.clone();
    let registration = child
        .on_with::<Ping, _>(
            observer_sync(move |_, _| {
                callback_hits.fetch_add(1, Ordering::SeqCst);
                Ok::<(), Infallible>(())
            }),
            ListenerOptions::default().once(),
        )
        .unwrap();

    root.emit::<Ping>(Routing::Scoped(sibling.scope()), 1)
        .await
        .unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    root.emit::<Ping>(Routing::Scoped(child.scope()), 2)
        .await
        .unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert!(!registration.remove());
}

#[tokio::test]
async fn prepend_order_is_semantic_for_notification_query_and_waterfall() {
    struct Notify;
    impl Event for Notify {
        const NAME: &'static str = "issue32/order-notify";
        type Args = ();
        type Output = ();
    }
    let ctx = Context::new();
    let order = Arc::new(Mutex::new(Vec::new()));
    for (label, options) in [
        ("append-a", ListenerOptions::default()),
        ("prepend-b", ListenerOptions::default().prepend()),
        ("prepend-c", ListenerOptions::default().prepend()),
        ("append-d", ListenerOptions::default()),
    ] {
        let order = order.clone();
        ctx.on_with::<Notify, _>(
            observer_sync(move |_, ()| {
                order.lock().push(label);
                Ok::<(), Infallible>(())
            }),
            options,
        )
        .unwrap();
    }
    ctx.emit::<Notify>(Routing::Unscoped, ()).await.unwrap();
    assert_eq!(
        &*order.lock(),
        &["prepend-c", "prepend-b", "append-a", "append-d"]
    );

    struct Ask;
    impl Event for Ask {
        const NAME: &'static str = "issue32/order-query";
        type Args = ();
        type Output = usize;
    }
    let ctx = Context::new();
    let order = Arc::new(Mutex::new(Vec::new()));
    for (label, options) in [
        ("append-a", ListenerOptions::default()),
        ("prepend-b", ListenerOptions::default().prepend()),
        ("prepend-c", ListenerOptions::default().prepend()),
        ("append-d", ListenerOptions::default()),
    ] {
        let order = order.clone();
        ctx.on_with::<Ask, _>(
            responder_sync(move |_, ()| {
                order.lock().push(label);
                Ok::<Option<usize>, Infallible>(None)
            }),
            options,
        )
        .unwrap();
    }
    assert!(matches!(
        ctx.query::<Ask>(Routing::Unscoped, ()).await.unwrap(),
        QueryOutcome::Miss
    ));
    assert_eq!(
        &*order.lock(),
        &["prepend-c", "prepend-b", "append-a", "append-d"]
    );

    struct MapOrder;
    impl Event for MapOrder {
        const NAME: &'static str = "issue32/order-waterfall";
        type Args = Vec<&'static str>;
        type Output = Vec<&'static str>;
    }
    let ctx = Context::new();
    for (label, options) in [
        ("append-a", ListenerOptions::default()),
        ("prepend-b", ListenerOptions::default().prepend()),
        ("prepend-c", ListenerOptions::default().prepend()),
        ("append-d", ListenerOptions::default()),
    ] {
        ctx.on_with::<MapOrder, _>(
            mapper_sync(move |_, mut value: Vec<&'static str>| {
                value.push(label);
                Ok::<_, Infallible>(value)
            }),
            options,
        )
        .unwrap();
    }
    let order = ctx
        .waterfall::<MapOrder, _, _, _>(Routing::Unscoped, Vec::new(), |value| async move {
            Ok::<_, Infallible>(value)
        })
        .await
        .unwrap();
    assert_eq!(order, ["prepend-c", "prepend-b", "append-a", "append-d"]);
}

#[tokio::test]
async fn once_returned_failure_is_not_restored_for_responder_mapper_or_around() {
    let responder = Context::new();
    let responder_hits = Arc::new(AtomicUsize::new(0));
    let hits = responder_hits.clone();
    responder
        .on_with::<Ping, _>(
            responder_sync(move |_, _| -> Result<Option<usize>, std::io::Error> {
                hits.fetch_add(1, Ordering::SeqCst);
                Err(std::io::Error::other("responder failed"))
            }),
            ListenerOptions::default().once(),
        )
        .unwrap();
    assert!(matches!(
        responder.query::<Ping>(Routing::Unscoped, 1).await,
        Err(DispatchError::Invocation(_))
    ));
    assert!(matches!(
        responder.query::<Ping>(Routing::Unscoped, 2).await.unwrap(),
        QueryOutcome::Miss
    ));
    assert_eq!(responder_hits.load(Ordering::SeqCst), 1);

    let mapper = Context::new();
    let mapper_hits = Arc::new(AtomicUsize::new(0));
    let hits = mapper_hits.clone();
    mapper
        .on_with::<Flow, _>(
            mapper_sync(move |_, _| -> Result<usize, std::io::Error> {
                hits.fetch_add(1, Ordering::SeqCst);
                Err(std::io::Error::other("mapper failed"))
            }),
            ListenerOptions::default().once(),
        )
        .unwrap();
    assert!(matches!(
        mapper
            .waterfall::<Flow, _, _, _>(Routing::Unscoped, 1, |value| async move {
                Ok::<_, Infallible>(value)
            })
            .await,
        Err(DispatchError::Invocation(_))
    ));
    assert_eq!(
        mapper
            .waterfall::<Flow, _, _, _>(Routing::Unscoped, 2, |value| async move {
                Ok::<_, Infallible>(value)
            })
            .await
            .unwrap(),
        2
    );
    assert_eq!(mapper_hits.load(Ordering::SeqCst), 1);

    let around_ctx = Context::new();
    let around_hits = Arc::new(AtomicUsize::new(0));
    let hits = around_hits.clone();
    around_ctx
        .on_with::<Flow, _>(
            around(move |_, _, _next| {
                let hits = hits.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    Err::<usize, std::io::Error>(std::io::Error::other("around failed"))
                }
            }),
            ListenerOptions::default().once(),
        )
        .unwrap();
    assert!(matches!(
        around_ctx
            .waterfall::<Flow, _, _, _>(Routing::Unscoped, 1, |value| async move {
                Ok::<_, Infallible>(value)
            })
            .await,
        Err(DispatchError::Invocation(_))
    ));
    assert_eq!(
        around_ctx
            .waterfall::<Flow, _, _, _>(Routing::Unscoped, 2, |value| async move {
                Ok::<_, Infallible>(value)
            })
            .await
            .unwrap(),
        2
    );
    assert_eq!(around_hits.load(Ordering::SeqCst), 1);
}

struct RemoveOnDrop(Option<ListenerRegistration>);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        if let Some(registration) = self.0.take() {
            let _ = registration.remove();
        }
    }
}

#[test]
fn listener_removal_drops_user_capture_outside_store_lock() {
    let ctx = Context::new();
    let sibling = ctx
        .on::<Ping, _>(observer_sync(|_, _| Ok::<(), Infallible>(())))
        .unwrap();
    let reentrant = RemoveOnDrop(Some(sibling));
    let victim = ctx
        .on::<Ping, _>(observer_sync(move |_, _| {
            let _ = &reentrant;
            Ok::<(), Infallible>(())
        }))
        .unwrap();

    assert!(common::deadlock_watchdog(
        "listener removal deadlocked while dropping a captured registration",
        move || victim.remove(),
    ));
}

#[tokio::test]
async fn remove_wins_against_an_unreached_once_snapshot() {
    let ctx = Context::new();
    let (block_started_tx, block_started_rx) = tokio::sync::oneshot::channel();
    let block_started = Arc::new(Mutex::new(Some(block_started_tx)));
    let block_release = Arc::new(tokio::sync::Notify::new());
    let started = block_started.clone();
    let release = block_release.clone();
    ctx.on::<Ping, _>(observer(move |_, _| {
        let started = started.clone();
        let release = release.clone();
        async move {
            if let Some(tx) = started.lock().take() {
                let _ = tx.send(());
            }
            release.notified().await;
            Ok::<(), Infallible>(())
        }
    }))
    .unwrap();

    let once_hits = Arc::new(AtomicUsize::new(0));
    let hits = once_hits.clone();
    let once = ctx
        .on_with::<Ping, _>(
            observer_sync(move |_, _| {
                hits.fetch_add(1, Ordering::SeqCst);
                Ok::<(), Infallible>(())
            }),
            ListenerOptions::default().once(),
        )
        .unwrap();
    let emitter = ctx.clone();
    let dispatch = tokio::spawn(async move { emitter.emit::<Ping>(Routing::Unscoped, 1).await });
    block_started_rx.await.unwrap();
    assert!(once.remove());
    block_release.notify_one();
    dispatch.await.unwrap().unwrap();
    assert_eq!(once_hits.load(Ordering::SeqCst), 0);
}

struct DrainPlugin {
    hits: Arc<AtomicUsize>,
    cleanup_started: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    cleanup_release: Arc<tokio::sync::Notify>,
}

impl Plugin for DrainPlugin {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = std::io::Error;

    fn name(&self) -> Cow<'_, str> {
        Cow::Borrowed("issue32-drain")
    }

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _: &()) -> Result<(), std::io::Error> {
        let hits = self.hits.clone();
        ctx.on::<Ping, _>(observer_sync(move |_, _| {
            hits.fetch_add(1, Ordering::SeqCst);
            Ok::<(), Infallible>(())
        }))
        .map_err(|error| std::io::Error::other(error.to_string()))?;
        let started = self.cleanup_started.clone();
        let release = self.cleanup_release.clone();
        ctx.effect(move || async move {
            if let Some(tx) = started.lock().take() {
                let _ = tx.send(());
            }
            release.notified().await;
        })
        .map_err(|error| std::io::Error::other(error.to_string()))?;
        Ok(())
    }
}

#[tokio::test]
async fn generation_close_keeps_occurrence_eligible_until_exact_cleanup_and_stale_cleanup_spares_duplicate()
 {
    let root = Context::new();
    let hits = Arc::new(AtomicUsize::new(0));
    let (cleanup_started_tx, cleanup_started_rx) = tokio::sync::oneshot::channel();
    let cleanup_release = Arc::new(tokio::sync::Notify::new());
    let fiber_handle = root
        .spawn(PreparedPlugin::from_input(
            DrainPlugin {
                hits: hits.clone(),
                cleanup_started: Arc::new(Mutex::new(Some(cleanup_started_tx))),
                cleanup_release: cleanup_release.clone(),
            },
            (),
        ))
        .await
        .unwrap();

    let disposal = tokio::spawn(async move { fiber_handle.dispose().await });
    cleanup_started_rx.await.unwrap();

    // The generation is closed, but its listener cleanup is still behind the
    // blocked later cleanup and therefore remains eligible.
    let duplicate_hits = hits.clone();
    let duplicate = root
        .on::<Ping, _>(observer_sync(move |_, _| {
            duplicate_hits.fetch_add(1, Ordering::SeqCst);
            Ok::<(), Infallible>(())
        }))
        .unwrap();
    root.emit::<Ping>(Routing::Unscoped, 1).await.unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 2);

    cleanup_release.notify_one();
    disposal.await.unwrap().unwrap();
    root.emit::<Ping>(Routing::Unscoped, 2).await.unwrap();
    assert_eq!(
        hits.load(Ordering::SeqCst),
        3,
        "stale old-generation cleanup must spare the later duplicate"
    );
    assert!(duplicate.remove());
}

struct LoadingPlugin {
    hits: Arc<AtomicUsize>,
    registered: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    release: Arc<tokio::sync::Notify>,
}

impl Plugin for LoadingPlugin {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = std::io::Error;

    fn name(&self) -> Cow<'_, str> {
        Cow::Borrowed("issue32-loading")
    }

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _: &()) -> Result<(), std::io::Error> {
        let hits = self.hits.clone();
        ctx.on::<Ping, _>(observer_sync(move |_, _| {
            hits.fetch_add(1, Ordering::SeqCst);
            Ok::<(), Infallible>(())
        }))
        .map_err(|error| std::io::Error::other(error.to_string()))?;
        if let Some(tx) = self.registered.lock().take() {
            let _ = tx.send(());
        }
        self.release.notified().await;
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn committed_loading_registration_is_immediately_eligible_and_creator_cancellation_cleans_it()
{
    let root = Context::new();
    let hits = Arc::new(AtomicUsize::new(0));
    let (registered_tx, registered_rx) = tokio::sync::oneshot::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let creator = {
        let root = root.clone();
        let hits = hits.clone();
        let release = release.clone();
        tokio::spawn(async move {
            root.spawn(PreparedPlugin::from_input(
                LoadingPlugin {
                    hits,
                    registered: Arc::new(Mutex::new(Some(registered_tx))),
                    release,
                },
                (),
            ))
            .await
        })
    };

    registered_rx.await.unwrap();
    root.emit::<Ping>(Routing::Unscoped, 1).await.unwrap();
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "Loading registration is eligible immediately after commit"
    );

    creator.abort();
    assert!(creator.await.unwrap_err().is_cancelled());
    let cleaned = common::bounded(2_000, async {
        loop {
            let before = hits.load(Ordering::SeqCst);
            root.emit::<Ping>(Routing::Unscoped, 2).await.unwrap();
            if hits.load(Ordering::SeqCst) == before {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(
        cleaned.is_some(),
        "creator cancellation must drive generation cleanup"
    );
}

struct DropProbe(Arc<AtomicUsize>);

impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct RegistrationControlPlugin {
    slot: Arc<Mutex<Option<ListenerRegistration>>>,
    drops: Arc<AtomicUsize>,
}

impl Plugin for RegistrationControlPlugin {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn name(&self) -> Cow<'_, str> {
        Cow::Borrowed("issue32-registration-control")
    }

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _: &()) -> Result<(), Infallible> {
        let probe = DropProbe(self.drops.clone());
        let registration = ctx
            .on::<Ping, _>(observer_sync(move |_, _| {
                let _ = &probe;
                Ok::<(), Infallible>(())
            }))
            .expect("live plugin generation accepts registration");
        *self.slot.lock() = Some(registration);
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_remove_and_generation_cleanup_arbitrate_one_exact_unregister() {
    for _round in 0..24 {
        let root = Context::new();
        let slot = Arc::new(Mutex::new(None));
        let drops = Arc::new(AtomicUsize::new(0));
        let fiber_handle = root
            .spawn(PreparedPlugin::from_input(
                RegistrationControlPlugin {
                    slot: slot.clone(),
                    drops: drops.clone(),
                },
                (),
            ))
            .await
            .unwrap();
        let registration = slot.lock().take().unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(3));

        let manual = {
            let barrier = barrier.clone();
            tokio::task::spawn_blocking(move || {
                barrier.wait();
                registration.remove()
            })
        };
        let cleanup = {
            let barrier = barrier.clone();
            tokio::spawn(async move {
                barrier.wait();
                fiber_handle.dispose().await
            })
        };
        barrier.wait();

        let _manual_won = manual.await.unwrap();
        cleanup.await.unwrap().unwrap();
        assert_eq!(
            drops.load(Ordering::SeqCst),
            1,
            "manual remove and generation cleanup must destroy one exact hook once"
        );
        root.emit::<Ping>(Routing::Unscoped, 1).await.unwrap();
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
}

struct CaptureContextPlugin {
    slot: Arc<Mutex<Option<Context>>>,
}

impl Plugin for CaptureContextPlugin {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn name(&self) -> Cow<'_, str> {
        Cow::Borrowed("issue32-capture")
    }

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _: &()) -> Result<(), Infallible> {
        *self.slot.lock() = Some(ctx);
        Ok(())
    }
}

#[tokio::test]
async fn closing_first_refuses_registration_without_a_listener_trace() {
    let root = Context::new();
    let slot = Arc::new(Mutex::new(None));
    let fiber_handle = root
        .spawn(PreparedPlugin::from_input(
            CaptureContextPlugin { slot: slot.clone() },
            (),
        ))
        .await
        .unwrap();
    let captured = slot.lock().take().unwrap();
    fiber_handle.dispose().await.unwrap();

    let hits = Arc::new(AtomicUsize::new(0));
    let callback_hits = hits.clone();
    let error = match captured.on::<Ping, _>(observer_sync(move |_, _| {
        callback_hits.fetch_add(1, Ordering::SeqCst);
        Ok::<(), Infallible>(())
    })) {
        Ok(_) => panic!("closing-first registration must be refused"),
        Err(error) => error,
    };
    assert!(matches!(error, ListenerRegistrationError::InactiveContext));
    root.emit::<Ping>(Routing::Unscoped, 1).await.unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn registration_racing_generation_close_never_strands_an_occurrence() {
    for _round in 0..24 {
        let root = Context::new();
        let slot = Arc::new(Mutex::new(None));
        let fiber_handle = root
            .spawn(PreparedPlugin::from_input(
                CaptureContextPlugin { slot: slot.clone() },
                (),
            ))
            .await
            .unwrap();
        let captured = slot.lock().take().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(std::sync::Barrier::new(3));

        let registration = {
            let barrier = barrier.clone();
            let hits = hits.clone();
            tokio::task::spawn_blocking(move || {
                barrier.wait();
                captured.on::<Ping, _>(observer_sync(move |_, _| {
                    hits.fetch_add(1, Ordering::SeqCst);
                    Ok::<(), Infallible>(())
                }))
            })
        };
        let disposal = {
            let barrier = barrier.clone();
            tokio::spawn(async move {
                barrier.wait();
                fiber_handle.dispose().await
            })
        };
        barrier.wait();

        let registration = registration.await.unwrap();
        disposal.await.unwrap().unwrap();
        root.emit::<Ping>(Routing::Unscoped, 1).await.unwrap();
        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "no listener may survive completed disposal"
        );

        match registration {
            Ok(registration) => {
                assert!(
                    !registration.remove(),
                    "commit-first was drained exactly once"
                );
            }
            Err(error) => assert!(matches!(error, ListenerRegistrationError::InactiveContext)),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_once_losses_are_neutral_for_responder_mapper_and_around() {
    let responder = Context::new();
    let responder_hits = Arc::new(AtomicUsize::new(0));
    let hits = responder_hits.clone();
    responder
        .on_with::<Ping, _>(
            responder_sync(move |_, value| {
                hits.fetch_add(1, Ordering::SeqCst);
                Ok::<_, Infallible>(Some(value + 10))
            }),
            ListenerOptions::default().once(),
        )
        .unwrap();
    let start = Arc::new(tokio::sync::Barrier::new(3));
    let left = {
        let ctx = responder.clone();
        let start = start.clone();
        tokio::spawn(async move {
            start.wait().await;
            ctx.query::<Ping>(Routing::Unscoped, 1).await.unwrap()
        })
    };
    let right = {
        let ctx = responder.clone();
        let start = start.clone();
        tokio::spawn(async move {
            start.wait().await;
            ctx.query::<Ping>(Routing::Unscoped, 2).await.unwrap()
        })
    };
    start.wait().await;
    let (left, right) = (left.await.unwrap(), right.await.unwrap());
    assert_eq!(responder_hits.load(Ordering::SeqCst), 1);
    assert!(matches!(left, QueryOutcome::Miss) ^ matches!(right, QueryOutcome::Miss));
    assert!(matches!(left, QueryOutcome::Answer(_)) ^ matches!(right, QueryOutcome::Answer(_)));

    let mapper = Context::new();
    let mapper_hits = Arc::new(AtomicUsize::new(0));
    let hits = mapper_hits.clone();
    mapper
        .on_with::<Flow, _>(
            mapper_sync(move |_, value| {
                hits.fetch_add(1, Ordering::SeqCst);
                Ok::<_, Infallible>(value + 10)
            }),
            ListenerOptions::default().once(),
        )
        .unwrap();
    let start = Arc::new(tokio::sync::Barrier::new(3));
    let run = |ctx: Context, start: Arc<tokio::sync::Barrier>, value| {
        tokio::spawn(async move {
            start.wait().await;
            ctx.waterfall::<Flow, _, _, _>(Routing::Unscoped, value, |tail| async move {
                Ok::<_, Infallible>(tail)
            })
            .await
            .unwrap()
        })
    };
    let left = run(mapper.clone(), start.clone(), 1);
    let right = run(mapper.clone(), start.clone(), 2);
    start.wait().await;
    let mut outcomes = [left.await.unwrap(), right.await.unwrap()];
    outcomes.sort_unstable();
    assert_eq!(mapper_hits.load(Ordering::SeqCst), 1);
    assert!(matches!(outcomes.as_slice(), [1, 12] | [2, 11]));

    let around_ctx = Context::new();
    let around_hits = Arc::new(AtomicUsize::new(0));
    let hits = around_hits.clone();
    around_ctx
        .on_with::<Flow, _>(
            around(
                move |_, value: usize, next: cordis_core::event::Next<Flow>| {
                    let hits = hits.clone();
                    async move {
                        hits.fetch_add(1, Ordering::SeqCst);
                        Ok::<_, Infallible>(next.call(value).await.unwrap() + 10)
                    }
                },
            ),
            ListenerOptions::default().once(),
        )
        .unwrap();
    let start = Arc::new(tokio::sync::Barrier::new(3));
    let run = |ctx: Context, start: Arc<tokio::sync::Barrier>, value| {
        tokio::spawn(async move {
            start.wait().await;
            ctx.waterfall::<Flow, _, _, _>(Routing::Unscoped, value, |tail| async move {
                Ok::<_, Infallible>(tail)
            })
            .await
            .unwrap()
        })
    };
    let left = run(around_ctx.clone(), start.clone(), 1);
    let right = run(around_ctx.clone(), start.clone(), 2);
    start.wait().await;
    let mut outcomes = [left.await.unwrap(), right.await.unwrap()];
    outcomes.sort_unstable();
    assert_eq!(around_hits.load(Ordering::SeqCst), 1);
    assert!(matches!(outcomes.as_slice(), [1, 12] | [2, 11]));
}
