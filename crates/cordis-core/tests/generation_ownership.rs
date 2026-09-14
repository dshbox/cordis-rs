//! Issue 54 evidence: one generation owns every retained core resource family.
//!
//! Local occurrence semantics belong to Issues 20/21/25/32/44. This suite is
//! deliberately cross-family: it proves the common generation gate, the two
//! publish-versus-close outcomes, cross-Fiber non-transfer, restart replacement,
//! ordinary-spawn non-ownership, and the permanent root exception.

use cordis_core::effect::{EffectRegistrationError, TaskRegistrationError};
use cordis_core::event::{ListenerRegistrationError, observer_sync};
use cordis_core::lifecycle::SpawnError;
use cordis_core::logger::{Exporter, Level, LogRecord};
use cordis_core::service::{ServiceControlError, ServiceLookupError, ServicePublishError};
use cordis_core::{
    Context, Event, FiberState, InjectSpec, Plugin, PreparedPlugin, Routing, Service,
};
use parking_lot::Mutex;
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context as TaskContext, Poll};

struct OwnershipEvent;
impl Event for OwnershipEvent {
    const NAME: &'static str = "issue54/ownership-event";
    type Args = ();
    type Output = ();
}

struct RootEvent;
impl Event for RootEvent {
    const NAME: &'static str = "issue54/root-event";
    type Args = ();
    type Output = ();
}

#[derive(Debug)]
struct LoadingService;
impl Service for LoadingService {
    const NAME: &'static str = "issue54/loading-service";
}

#[derive(Debug)]
struct ActiveService;
impl Service for ActiveService {
    const NAME: &'static str = "issue54/active-service";
}

#[derive(Debug)]
struct RootService;
impl Service for RootService {
    const NAME: &'static str = "issue54/root-service";
}

#[derive(Debug)]
struct RefusedService;
impl Service for RefusedService {
    const NAME: &'static str = "issue54/refused-service";
}

#[derive(Debug)]
struct RaceService;
impl Service for RaceService {
    const NAME: &'static str = "issue54/race-service";
}

#[derive(Debug)]
struct CrossService;
impl Service for CrossService {
    const NAME: &'static str = "issue54/cross-service";
}

#[derive(Debug)]
struct ChildService;
impl Service for ChildService {
    const NAME: &'static str = "issue54/child-service";
}

#[derive(Debug)]
struct GateDependency;
impl Service for GateDependency {
    const NAME: &'static str = "issue54/gate-dependency";
}

#[derive(Default)]
struct CountingExporter {
    hits: AtomicUsize,
}
impl Exporter for CountingExporter {
    fn export(&self, _record: &LogRecord) {
        self.hits.fetch_add(1, Ordering::SeqCst);
    }
    fn default_level(&self) -> Level {
        Level::Debug
    }
}
impl CountingExporter {
    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

struct PollProbe(Arc<AtomicUsize>);
impl Future for PollProbe {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Poll::Ready(())
    }
}

#[derive(Clone)]
struct CapturePlugin {
    slot: Arc<Mutex<Option<Context>>>,
}
impl Plugin for CapturePlugin {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _: &()) -> Result<(), Infallible> {
        *self.slot.lock() = Some(ctx);
        Ok(())
    }
}

struct GatePlugin {
    slot: Arc<Mutex<Option<Context>>>,
    fail: Arc<AtomicBool>,
}
impl Plugin for GatePlugin {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = std::io::Error;

    fn inject(&self) -> InjectSpec {
        InjectSpec::none().require(GateDependency::NAME)
    }

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _: &()) -> Result<(), std::io::Error> {
        *self.slot.lock() = Some(ctx);
        if self.fail.load(Ordering::SeqCst) {
            Err(std::io::Error::other("issue54 forced apply failure"))
        } else {
            Ok(())
        }
    }
}

struct LoadingProbe {
    effect_cleaned: Arc<AtomicUsize>,
    event_hits: Arc<AtomicUsize>,
    exporter: Arc<CountingExporter>,
    task_started: Arc<tokio::sync::Notify>,
}
impl Plugin for LoadingProbe {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _: &()) -> Result<(), Infallible> {
        let cleaned = self.effect_cleaned.clone();
        ctx.effect_sync(move || {
            cleaned.fetch_add(1, Ordering::SeqCst);
        })
        .expect("Loading admits pure Effect");

        let _publication = ctx
            .provide(Arc::new(LoadingService))
            .expect("Loading admits Service publication");

        let hits = self.event_hits.clone();
        ctx.on::<OwnershipEvent, _>(observer_sync(move |_, _| {
            hits.fetch_add(1, Ordering::SeqCst);
            Ok::<(), Infallible>(())
        }))
        .expect("Loading admits Event listener registration");

        ctx.add_exporter(self.exporter.clone())
            .expect("Loading admits Logger exporter registration");

        let started = self.task_started.clone();
        ctx.run(async move {
            started.notify_one();
        })
        .expect("Loading admits cooperative task registration");
        Ok(())
    }
}

struct ChildOwnsResource {
    cleaned: Arc<AtomicUsize>,
}
impl Plugin for ChildOwnsResource {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _: &()) -> Result<(), Infallible> {
        let _publication = ctx.provide(Arc::new(ChildService)).unwrap();
        let cleaned = self.cleaned.clone();
        ctx.effect_sync(move || {
            cleaned.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        Ok(())
    }
}

fn prepared<P>(plugin: P) -> PreparedPlugin
where
    P: Plugin<Config = (), Input = (), PrepareError = Infallible>,
{
    PreparedPlugin::from_input(plugin, ())
}

async fn scoped_context(root: &Context) -> (cordis_core::FiberHandle, Context) {
    let slot = Arc::new(Mutex::new(None));
    let fiber_handle = root
        .spawn(prepared(CapturePlugin { slot: slot.clone() }))
        .await
        .unwrap();
    let ctx = slot.lock().clone().expect("apply captured its Context");
    (fiber_handle, ctx)
}

async fn assert_core_retained_refusal(ctx: &Context, root: &Context) {
    let effect_ran = Arc::new(AtomicUsize::new(0));
    let effect = ctx.effect_sync({
        let effect_ran = effect_ran.clone();
        move || {
            effect_ran.fetch_add(1, Ordering::SeqCst);
        }
    });
    assert!(matches!(
        effect,
        Err(EffectRegistrationError::InactiveContext)
    ));
    assert_eq!(effect_ran.load(Ordering::SeqCst), 0);

    let publication = ctx.provide(Arc::new(RefusedService));
    assert!(matches!(
        publication,
        Err(ServicePublishError::InactiveContext)
    ));
    assert!(matches!(
        root.try_service::<RefusedService>(),
        Err(ServiceLookupError::Unavailable { .. })
    ));

    let event_hits = Arc::new(AtomicUsize::new(0));
    let listener = ctx.on::<OwnershipEvent, _>(observer_sync({
        let event_hits = event_hits.clone();
        move |_, _| {
            event_hits.fetch_add(1, Ordering::SeqCst);
            Ok::<(), Infallible>(())
        }
    }));
    assert!(matches!(
        listener,
        Err(ListenerRegistrationError::InactiveContext)
    ));
    root.emit::<OwnershipEvent>(Routing::Unscoped, ())
        .await
        .unwrap();
    assert_eq!(event_hits.load(Ordering::SeqCst), 0);

    let exporter = Arc::new(CountingExporter::default());
    assert!(ctx.add_exporter(exporter.clone()).is_err());
    root.logger().info("issue54 refused exporter probe");
    assert_eq!(exporter.hits(), 0);

    let task_polls = Arc::new(AtomicUsize::new(0));
    assert_eq!(
        ctx.run(PollProbe(task_polls.clone())),
        Err(TaskRegistrationError::InactiveContext)
    );
    assert_eq!(task_polls.load(Ordering::SeqCst), 0);

    let fibers_before = root.runtime_snapshot().fibers().len();
    let child_slot = Arc::new(Mutex::new(None));
    let spawned = ctx
        .spawn(prepared(CapturePlugin {
            slot: child_slot.clone(),
        }))
        .await;
    match spawned {
        Err(SpawnError::InactiveContext) => {}
        Err(other) => panic!("unexpected spawn refusal: {other}"),
        Ok(_) => panic!("closed generation allocated and delivered a child Fiber"),
    }
    assert!(child_slot.lock().is_none(), "refused spawn never ran apply");
    assert_eq!(
        root.runtime_snapshot().fibers().len(),
        fibers_before,
        "refused spawn leaves no resident allocation"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loading_active_and_permanent_root_admit_and_own_their_core_resources() {
    let root = Context::new();
    let loading_effect = Arc::new(AtomicUsize::new(0));
    let loading_events = Arc::new(AtomicUsize::new(0));
    let loading_exporter = Arc::new(CountingExporter::default());
    let loading_task = Arc::new(tokio::sync::Notify::new());

    let loading = root
        .spawn(prepared(LoadingProbe {
            effect_cleaned: loading_effect.clone(),
            event_hits: loading_events.clone(),
            exporter: loading_exporter.clone(),
            task_started: loading_task.clone(),
        }))
        .await
        .unwrap();
    assert_eq!(loading.state(), FiberState::Active);
    loading_task.notified().await;
    assert!(root.try_service::<LoadingService>().is_ok());
    root.emit::<OwnershipEvent>(Routing::Unscoped, ())
        .await
        .unwrap();
    assert_eq!(loading_events.load(Ordering::SeqCst), 1);
    root.logger().info("loading-owned exporter is live");
    assert_eq!(loading_exporter.hits(), 1);
    assert_eq!(loading_effect.load(Ordering::SeqCst), 0);

    loading.dispose().await.unwrap();
    assert!(matches!(
        root.try_service::<LoadingService>(),
        Err(ServiceLookupError::Unavailable { .. })
    ));
    root.emit::<OwnershipEvent>(Routing::Unscoped, ())
        .await
        .unwrap();
    assert_eq!(loading_events.load(Ordering::SeqCst), 1);
    root.logger().info("loading-owned exporter is gone");
    assert_eq!(loading_exporter.hits(), 1);
    assert_eq!(loading_effect.load(Ordering::SeqCst), 1);

    let (active_fiber_handle, active) = scoped_context(&root).await;
    let active_effect = Arc::new(AtomicUsize::new(0));
    active
        .effect_sync({
            let active_effect = active_effect.clone();
            move || {
                active_effect.fetch_add(1, Ordering::SeqCst);
            }
        })
        .unwrap();
    let _active_publication = active.provide(Arc::new(ActiveService)).unwrap();
    let active_events = Arc::new(AtomicUsize::new(0));
    active
        .on::<OwnershipEvent, _>(observer_sync({
            let active_events = active_events.clone();
            move |_, _| {
                active_events.fetch_add(1, Ordering::SeqCst);
                Ok::<(), Infallible>(())
            }
        }))
        .unwrap();
    let active_exporter = Arc::new(CountingExporter::default());
    active.add_exporter(active_exporter.clone()).unwrap();
    let active_task = Arc::new(tokio::sync::Notify::new());
    active
        .run({
            let active_task = active_task.clone();
            async move { active_task.notify_one() }
        })
        .unwrap();
    active_task.notified().await;

    let root_effect = root.effect_sync(|| {}).unwrap();
    let _root_publication = root.provide(Arc::new(RootService)).unwrap();
    let root_events = Arc::new(AtomicUsize::new(0));
    root.on::<RootEvent, _>(observer_sync({
        let root_events = root_events.clone();
        move |_, _| {
            root_events.fetch_add(1, Ordering::SeqCst);
            Ok::<(), Infallible>(())
        }
    }))
    .unwrap();
    let root_exporter = Arc::new(CountingExporter::default());
    root.add_exporter(root_exporter.clone()).unwrap();
    root.run(async {}).unwrap();

    active_fiber_handle.dispose().await.unwrap();
    assert_eq!(active_effect.load(Ordering::SeqCst), 1);
    assert!(matches!(
        root.try_service::<ActiveService>(),
        Err(ServiceLookupError::Unavailable { .. })
    ));
    assert!(root.try_service::<RootService>().is_ok());
    root.emit::<RootEvent>(Routing::Unscoped, ()).await.unwrap();
    assert_eq!(root_events.load(Ordering::SeqCst), 1);
    root.logger()
        .info("root remains open after ordinary Fiber disposal");
    assert!(root_exporter.hits() > 0);
    assert!(
        root_effect.disarm(),
        "the permanent root still owns this cleanup"
    );

    let late_root_effect = root.effect_sync(|| {}).unwrap();
    assert!(
        late_root_effect.disarm(),
        "root admits new registration after child disposal"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pending_failed_closing_and_disposed_refuse_every_core_retained_family() {
    // Stable Pending: first run Active so we can retain its Context, then withdraw
    // an exact required publication and drive the Fiber to stable Pending.
    let pending_root = Context::new();
    let dependency = pending_root.provide(Arc::new(GateDependency)).unwrap();
    let pending_slot = Arc::new(Mutex::new(None));
    let pending = pending_root
        .spawn(prepared(GatePlugin {
            slot: pending_slot.clone(),
            fail: Arc::new(AtomicBool::new(false)),
        }))
        .await
        .unwrap();
    let pending_ctx = pending_slot.lock().clone().unwrap();
    dependency.remove().unwrap();
    assert_eq!(pending.ready().await.unwrap(), FiberState::Pending);
    assert_core_retained_refusal(&pending_ctx, &pending_root).await;
    pending.dispose().await.unwrap();

    // Stable Failed: explicit restart commits generation replacement, the new
    // apply captures the same Fiber Context, then failure rollback completes.
    let failed_root = Context::new();
    let _dependency = failed_root.provide(Arc::new(GateDependency)).unwrap();
    let failed_slot = Arc::new(Mutex::new(None));
    let fail = Arc::new(AtomicBool::new(false));
    let failed = failed_root
        .spawn(prepared(GatePlugin {
            slot: failed_slot.clone(),
            fail: fail.clone(),
        }))
        .await
        .unwrap();
    fail.store(true, Ordering::SeqCst);
    assert!(failed.restart().await.is_err());
    assert_eq!(failed.state(), FiberState::Failed);
    let failed_ctx = failed_slot.lock().clone().unwrap();
    assert_core_retained_refusal(&failed_ctx, &failed_root).await;
    failed.dispose().await.unwrap();

    // Closing: a newest blocking cleanup gives a deterministic point after the
    // generation gate has closed but before the terminal drain is complete.
    let closing_root = Context::new();
    let (closing_fiber_handle, closing_ctx) = scoped_context(&closing_root).await;
    let cleanup_started = Arc::new(tokio::sync::Notify::new());
    let cleanup_release = Arc::new(tokio::sync::Notify::new());
    closing_ctx
        .effect({
            let cleanup_started = cleanup_started.clone();
            let cleanup_release = cleanup_release.clone();
            move || async move {
                cleanup_started.notify_one();
                cleanup_release.notified().await;
            }
        })
        .unwrap();
    let disposer = tokio::spawn({
        let closing_fiber_handle = closing_fiber_handle.clone();
        async move { closing_fiber_handle.dispose().await }
    });
    cleanup_started.notified().await;
    assert_core_retained_refusal(&closing_ctx, &closing_root).await;
    cleanup_release.notify_one();
    disposer.await.unwrap().unwrap();

    assert_eq!(closing_fiber_handle.state(), FiberState::Disposed);
    assert_core_retained_refusal(&closing_ctx, &closing_root).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_core_family_publish_versus_close_has_only_refusal_or_owned_commit() {
    // Pure Effect.
    {
        let root = Context::new();
        let (fiber_handle, ctx) = scoped_context(&root).await;
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let ran = Arc::new(AtomicUsize::new(0));
        let register = tokio::spawn({
            let barrier = barrier.clone();
            let ran = ran.clone();
            async move {
                barrier.wait().await;
                ctx.effect_sync(move || {
                    ran.fetch_add(1, Ordering::SeqCst);
                })
            }
        });
        let close = tokio::spawn({
            let barrier = barrier.clone();
            let fiber_handle = fiber_handle.clone();
            async move {
                barrier.wait().await;
                fiber_handle.dispose().await.unwrap();
            }
        });
        barrier.wait().await;
        let outcome = register.await.unwrap();
        close.await.unwrap();
        match outcome {
            Ok(control) => {
                assert_eq!(ran.load(Ordering::SeqCst), 1);
                assert!(
                    !control.disarm(),
                    "generation drain already claimed the commit"
                );
            }
            Err(EffectRegistrationError::InactiveContext) => {
                assert_eq!(ran.load(Ordering::SeqCst), 0);
            }
            Err(other) => panic!("unexpected Effect race refusal: {other}"),
        }
    }

    // Service publication.
    {
        let root = Context::new();
        let (fiber_handle, ctx) = scoped_context(&root).await;
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let register = tokio::spawn({
            let barrier = barrier.clone();
            async move {
                barrier.wait().await;
                ctx.provide(Arc::new(RaceService))
            }
        });
        let close = tokio::spawn({
            let barrier = barrier.clone();
            let fiber_handle = fiber_handle.clone();
            async move {
                barrier.wait().await;
                fiber_handle.dispose().await.unwrap();
            }
        });
        barrier.wait().await;
        let outcome = register.await.unwrap();
        close.await.unwrap();
        assert!(matches!(
            root.try_service::<RaceService>(),
            Err(ServiceLookupError::Unavailable { .. })
        ));
        match outcome {
            Ok(control) => assert!(matches!(
                control.remove(),
                Err(ServiceControlError::StalePublication { .. })
            )),
            Err(ServicePublishError::InactiveContext) => {}
            Err(other) => panic!("unexpected Service race refusal: {other}"),
        }
    }

    // Event listener registration.
    {
        let root = Context::new();
        let (fiber_handle, ctx) = scoped_context(&root).await;
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let hits = Arc::new(AtomicUsize::new(0));
        let register = tokio::spawn({
            let barrier = barrier.clone();
            let hits = hits.clone();
            async move {
                barrier.wait().await;
                ctx.on::<OwnershipEvent, _>(observer_sync(move |_, _| {
                    hits.fetch_add(1, Ordering::SeqCst);
                    Ok::<(), Infallible>(())
                }))
            }
        });
        let close = tokio::spawn({
            let barrier = barrier.clone();
            let fiber_handle = fiber_handle.clone();
            async move {
                barrier.wait().await;
                fiber_handle.dispose().await.unwrap();
            }
        });
        barrier.wait().await;
        let outcome = register.await.unwrap();
        close.await.unwrap();
        root.emit::<OwnershipEvent>(Routing::Unscoped, ())
            .await
            .unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        match outcome {
            Ok(control) => assert!(
                !control.remove(),
                "generation cleanup removed the occurrence"
            ),
            Err(ListenerRegistrationError::InactiveContext) => {}
            Err(other) => panic!("unexpected Event race refusal: {other}"),
        }
    }

    // Logger exporter registration.
    {
        let root = Context::new();
        let (fiber_handle, ctx) = scoped_context(&root).await;
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let exporter = Arc::new(CountingExporter::default());
        let register = tokio::spawn({
            let barrier = barrier.clone();
            let exporter = exporter.clone();
            async move {
                barrier.wait().await;
                ctx.add_exporter(exporter)
            }
        });
        let close = tokio::spawn({
            let barrier = barrier.clone();
            let fiber_handle = fiber_handle.clone();
            async move {
                barrier.wait().await;
                fiber_handle.dispose().await.unwrap();
            }
        });
        barrier.wait().await;
        let outcome = register.await.unwrap();
        close.await.unwrap();
        root.logger().info("post-close exporter race probe");
        assert_eq!(exporter.hits(), 0);
        if let Ok(control) = outcome {
            assert!(!control.remove(), "generation cleanup removed the exporter");
        }
    }

    // Cooperative task registration.
    {
        let root = Context::new();
        let (fiber_handle, ctx) = scoped_context(&root).await;
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let polls = Arc::new(AtomicUsize::new(0));
        let register = tokio::spawn({
            let barrier = barrier.clone();
            let polls = polls.clone();
            async move {
                barrier.wait().await;
                ctx.run(PollProbe(polls))
            }
        });
        let close = tokio::spawn({
            let barrier = barrier.clone();
            let fiber_handle = fiber_handle.clone();
            async move {
                barrier.wait().await;
                fiber_handle.dispose().await.unwrap();
            }
        });
        barrier.wait().await;
        let outcome = register.await.unwrap();
        close.await.unwrap();
        match outcome {
            Ok(()) => assert_eq!(polls.load(Ordering::SeqCst), 1),
            Err(TaskRegistrationError::InactiveContext) => {
                assert_eq!(polls.load(Ordering::SeqCst), 0)
            }
            Err(other) => panic!("unexpected task race refusal: {other}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cross_fiber_use_does_not_transfer_owner_restart_replaces_and_spawn_is_not_parenthood() {
    let root = Context::new();
    let (owner, owner_ctx) = scoped_context(&root).await;
    let (user, user_ctx) = scoped_context(&root).await;

    let effect_cleaned = Arc::new(AtomicUsize::new(0));
    let event_hits = Arc::new(AtomicUsize::new(0));
    let exporter = Arc::new(CountingExporter::default());
    let owner_task_started = Arc::new(tokio::sync::Notify::new());
    let owner_task_finished = Arc::new(AtomicUsize::new(0));
    let cross_use_done = Arc::new(tokio::sync::Notify::new());

    // The code below executes as a task owned by `user`, but every retained
    // registration is made through `owner_ctx`. Execution attribution must not
    // rewrite the cleanup owner selected by that Context.
    user_ctx
        .run({
            let owner_ctx = owner_ctx.clone();
            let effect_cleaned = effect_cleaned.clone();
            let event_hits = event_hits.clone();
            let exporter = exporter.clone();
            let owner_task_started = owner_task_started.clone();
            let owner_task_finished = owner_task_finished.clone();
            let cross_use_done = cross_use_done.clone();
            async move {
                owner_ctx
                    .effect_sync(move || {
                        effect_cleaned.fetch_add(1, Ordering::SeqCst);
                    })
                    .unwrap();
                let _publication = owner_ctx.provide(Arc::new(CrossService)).unwrap();
                owner_ctx
                    .on::<OwnershipEvent, _>(observer_sync(move |_, _| {
                        event_hits.fetch_add(1, Ordering::SeqCst);
                        Ok::<(), Infallible>(())
                    }))
                    .unwrap();
                owner_ctx.add_exporter(exporter).unwrap();

                let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
                let release_tx = Arc::new(Mutex::new(Some(release_tx)));
                owner_ctx
                    .run({
                        let owner_task_started = owner_task_started.clone();
                        let owner_task_finished = owner_task_finished.clone();
                        async move {
                            owner_task_started.notify_one();
                            let _ = release_rx.await;
                            owner_task_finished.fetch_add(1, Ordering::SeqCst);
                        }
                    })
                    .unwrap();
                owner_ctx
                    .effect_sync(move || {
                        if let Some(tx) = release_tx.lock().take() {
                            let _ = tx.send(());
                        }
                    })
                    .unwrap();
                cross_use_done.notify_one();
            }
        })
        .unwrap();

    cross_use_done.notified().await;
    owner_task_started.notified().await;
    user.dispose().await.unwrap();

    assert_eq!(effect_cleaned.load(Ordering::SeqCst), 0);
    assert_eq!(owner_task_finished.load(Ordering::SeqCst), 0);
    assert!(root.try_service::<CrossService>().is_ok());
    root.emit::<OwnershipEvent>(Routing::Unscoped, ())
        .await
        .unwrap();
    assert_eq!(event_hits.load(Ordering::SeqCst), 1);
    root.logger().info("resource used after using Fiber closed");
    let exporter_before_restart = exporter.hits();
    assert!(exporter_before_restart > 0);

    owner.restart().await.unwrap();
    assert_eq!(effect_cleaned.load(Ordering::SeqCst), 1);
    assert_eq!(owner_task_finished.load(Ordering::SeqCst), 1);
    assert!(matches!(
        root.try_service::<CrossService>(),
        Err(ServiceLookupError::Unavailable { .. })
    ));
    root.emit::<OwnershipEvent>(Routing::Unscoped, ())
        .await
        .unwrap();
    assert_eq!(event_hits.load(Ordering::SeqCst), 1);
    root.logger()
        .info("old generation exporter is gone after restart");
    assert_eq!(exporter.hits(), exporter_before_restart);

    // The replacement generation is open and can own fresh resources.
    let replacement_effect = Arc::new(AtomicUsize::new(0));
    owner_ctx
        .effect_sync({
            let replacement_effect = replacement_effect.clone();
            move || {
                replacement_effect.fetch_add(1, Ordering::SeqCst);
            }
        })
        .unwrap();

    // A child spawned from the owner Context is resident independently. Parent
    // disposal is not a cleanup edge and must not touch the child's generation.
    let child_cleaned = Arc::new(AtomicUsize::new(0));
    let child = owner_ctx
        .spawn(prepared(ChildOwnsResource {
            cleaned: child_cleaned.clone(),
        }))
        .await
        .unwrap();
    assert!(root.try_service::<ChildService>().is_ok());

    owner.dispose().await.unwrap();
    assert_eq!(replacement_effect.load(Ordering::SeqCst), 1);
    assert_eq!(child_cleaned.load(Ordering::SeqCst), 0);
    assert!(
        root.try_service::<ChildService>().is_ok(),
        "ordinary spawn creates no parent cleanup ownership"
    );

    child.dispose().await.unwrap();
    assert_eq!(child_cleaned.load(Ordering::SeqCst), 1);
    assert!(matches!(
        root.try_service::<ChildService>(),
        Err(ServiceLookupError::Unavailable { .. })
    ));
}
