//! Issue 53 contract evidence for Loader pre-delivery result handoff ownership.

use std::convert::Infallible;
use std::error::Error;
use std::fmt;
use std::future::{Future, ready};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use cordis_core::lifecycle::FiberRole;
use cordis_core::logger::{BufferExporter, Level};
use cordis_core::{Context, FiberState, Plugin, PreparedPlugin};
use cordis_loader::outcome::{EntryOutcome, LoaderFailure};
use cordis_loader::plan::{LoadPlan, LoadPlanBuilder, PluginEntry};
use cordis_loader::resolver::PluginRequest;
use tokio::sync::Notify;

fn plugin(key: &str) -> PluginEntry {
    PluginEntry {
        key: Some(key.to_owned()),
        name: None,
        config: serde_json::Value::Null,
        disabled: false,
        inject: Vec::new(),
        isolate: Vec::new(),
    }
}

#[derive(Debug)]
struct ResolverError;
impl fmt::Display for ResolverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("resolver failed")
    }
}
impl Error for ResolverError {}

#[derive(Debug)]
struct CleanupError;
impl fmt::Display for CleanupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("cleanup failed")
    }
}
impl Error for CleanupError {}

#[derive(Clone, Copy)]
enum CleanupMode {
    Ok,
    Error,
    Panic,
}

#[derive(Clone)]
struct CleanupProbe {
    mode: CleanupMode,
    order: Arc<AtomicUsize>,
    observed: Arc<AtomicUsize>,
}

impl CleanupProbe {
    fn new(mode: CleanupMode, order: Arc<AtomicUsize>) -> Self {
        Self {
            mode,
            order,
            observed: Arc::new(AtomicUsize::new(0)),
        }
    }

    async fn wait(&self) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.observed_order() == 0 {
            assert!(Instant::now() < deadline, "expected cleanup did not run");
            tokio::task::yield_now().await;
        }
    }

    fn observed_order(&self) -> usize {
        self.observed.load(Ordering::SeqCst)
    }
}

struct CleanupPlugin(CleanupProbe);
impl Plugin for CleanupPlugin {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn apply(&self, ctx: Context, _: &()) -> impl Future<Output = Result<(), Infallible>> + Send {
        let probe = self.0.clone();
        drop(
            ctx.effect_sync(move || -> Result<(), CleanupError> {
                let position = probe.order.fetch_add(1, Ordering::SeqCst) + 1;
                probe.observed.store(position, Ordering::SeqCst);
                match probe.mode {
                    CleanupMode::Ok => Ok(()),
                    CleanupMode::Error => Err(CleanupError),
                    CleanupMode::Panic => panic!("cleanup panic"),
                }
            })
            .expect("active apply generation admits cleanup"),
        );
        ready(Ok(()))
    }
}

struct PanicOnInputDrop(Arc<AtomicUsize>);

impl Drop for PanicOnInputDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("last input reference dropped during loader rollback");
    }
}

struct PanicInputPlugin(CleanupProbe);

impl Plugin for PanicInputPlugin {
    type Config = ();
    type Input = PanicOnInputDrop;
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<PanicOnInputDrop, Infallible> {
        unreachable!("the resolver supplies the prepared input")
    }

    fn apply(
        &self,
        ctx: Context,
        _: &PanicOnInputDrop,
    ) -> impl Future<Output = Result<(), Infallible>> + Send {
        let probe = self.0.clone();
        async move { CleanupPlugin(probe).apply(ctx, &()).await }
    }
}

#[derive(Clone)]
struct BlockProbe {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

impl BlockProbe {
    fn new() -> Self {
        Self {
            entered: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
        }
    }
}

struct BlockingPlugin(BlockProbe);
impl Plugin for BlockingPlugin {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn apply(&self, _: Context, _: &()) -> impl Future<Output = Result<(), Infallible>> + Send {
        let entered = self.0.entered.clone();
        let release = self.0.release.clone();
        async move {
            entered.notify_one();
            release.notified().await;
            Ok(())
        }
    }
}

fn plan(keys: &[&str]) -> LoadPlan {
    let mut builder = LoadPlanBuilder::new();
    for key in keys {
        builder.add_plugin(None, plugin(key)).unwrap();
    }
    builder.finish().unwrap()
}

async fn wait_for_root_only(ctx: &Context) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while ctx.runtime_snapshot().fibers().len() != 1 {
        assert!(
            Instant::now() < deadline,
            "framework-owned rollback did not reach exact residency unlink"
        );
        tokio::task::yield_now().await;
    }
}

async fn abort_after_blocker_enters(
    ctx: Context,
    plan: LoadPlan,
    probes: Vec<(&'static str, CleanupProbe)>,
    blocker: BlockProbe,
) {
    let resolver_probes = probes.clone();
    let blocker_for_resolver = blocker.clone();
    let load_ctx = ctx.clone();
    let load = tokio::spawn(async move {
        let resolver =
            move |request: PluginRequest<'_>| -> Result<Option<PreparedPlugin>, ResolverError> {
                let key = request.resolve_key();
                if key == "block" {
                    return Ok(Some(PreparedPlugin::from_input(
                        BlockingPlugin(blocker_for_resolver.clone()),
                        (),
                    )));
                }
                let probe = resolver_probes
                    .iter()
                    .find_map(|(probe_key, probe)| (*probe_key == key).then(|| probe.clone()))
                    .expect("test resolver knows successful key");
                Ok(Some(PreparedPlugin::from_input(CleanupPlugin(probe), ())))
            };
        plan.load(&load_ctx, &resolver).await
    });

    blocker.entered.notified().await;
    load.abort();
    let cancelled = load.await;
    assert!(matches!(cancelled, Err(ref error) if error.is_cancelled()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abandonment_after_one_success_completes_rollback_without_repolling() {
    let ctx = Context::new();
    let order = Arc::new(AtomicUsize::new(0));
    let first = CleanupProbe::new(CleanupMode::Ok, order);
    let blocker = BlockProbe::new();

    abort_after_blocker_enters(
        ctx.clone(),
        plan(&["first", "block"]),
        vec![("first", first.clone())],
        blocker,
    )
    .await;

    first.wait().await;
    assert_eq!(first.observed_order(), 1);
    wait_for_root_only(&ctx).await;
}

#[test]
fn abandoned_handoff_survives_runtime_shutdown() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let ctx = Context::new();
    let order = Arc::new(AtomicUsize::new(0));
    let first = CleanupProbe::new(CleanupMode::Ok, order);
    let blocker = BlockProbe::new();
    let resolver_probe = first.clone();
    let blocker_for_resolver = blocker.clone();
    let load_ctx = ctx.clone();
    let load_plan = plan(&["first", "block"]);

    runtime.handle().spawn(async move {
        let resolver =
            move |request: PluginRequest<'_>| -> Result<Option<PreparedPlugin>, ResolverError> {
                if request.resolve_key() == "block" {
                    Ok(Some(PreparedPlugin::from_input(
                        BlockingPlugin(blocker_for_resolver.clone()),
                        (),
                    )))
                } else {
                    Ok(Some(PreparedPlugin::from_input(
                        CleanupPlugin(resolver_probe.clone()),
                        (),
                    )))
                }
            };
        let _ = load_plan.load(&load_ctx, &resolver).await;
    });

    runtime.block_on(blocker.entered.notified());
    runtime.shutdown_background();

    let deadline = Instant::now() + Duration::from_secs(2);
    while first.observed_order() == 0 && Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert_eq!(
        first.observed_order(),
        1,
        "pre-delivery successful Fiber must roll back after executor shutdown"
    );

    let observer = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    observer.block_on(wait_for_root_only(&ctx));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abandonment_rolls_back_reverse_success_order_attempt_all_across_cleanup_failure_and_panic()
{
    let ctx = Context::new();
    let order = Arc::new(AtomicUsize::new(0));
    let first = CleanupProbe::new(CleanupMode::Ok, order.clone());
    let second = CleanupProbe::new(CleanupMode::Error, order.clone());
    let third = CleanupProbe::new(CleanupMode::Panic, order);
    let blocker = BlockProbe::new();

    abort_after_blocker_enters(
        ctx.clone(),
        plan(&["first", "second", "third", "block"]),
        vec![
            ("first", first.clone()),
            ("second", second.clone()),
            ("third", third.clone()),
        ],
        blocker,
    )
    .await;

    third.wait().await;
    second.wait().await;
    first.wait().await;
    assert_eq!(third.observed_order(), 1, "last success rolls back first");
    assert_eq!(second.observed_order(), 2, "rollback continues after panic");
    assert_eq!(
        first.observed_order(),
        3,
        "rollback continues after returned error"
    );
    wait_for_root_only(&ctx).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abandonment_continues_after_last_input_drop_panics_between_members() {
    let ctx = Context::new();
    let buffer = Arc::new(BufferExporter::new(8, Level::Warn).unwrap());
    let _exporter = ctx.add_exporter(buffer.clone()).unwrap();
    let order = Arc::new(AtomicUsize::new(0));
    let first = CleanupProbe::new(CleanupMode::Ok, order.clone());
    let second = CleanupProbe::new(CleanupMode::Ok, order);
    let input_drops = Arc::new(AtomicUsize::new(0));
    let blocker = BlockProbe::new();
    let load_ctx = ctx.clone();
    let first_for_resolver = first.clone();
    let second_for_resolver = second.clone();
    let drops_for_resolver = input_drops.clone();
    let blocker_for_resolver = blocker.clone();

    let load = tokio::spawn(async move {
        let resolver = move |request: PluginRequest<'_>| {
            let prepared = match request.resolve_key() {
                "first" => {
                    PreparedPlugin::from_input(CleanupPlugin(first_for_resolver.clone()), ())
                }
                "second" => PreparedPlugin::from_input(
                    PanicInputPlugin(second_for_resolver.clone()),
                    PanicOnInputDrop(drops_for_resolver.clone()),
                ),
                "block" => {
                    PreparedPlugin::from_input(BlockingPlugin(blocker_for_resolver.clone()), ())
                }
                key => panic!("unexpected resolver key: {key}"),
            };
            Ok::<_, ResolverError>(Some(prepared))
        };
        plan(&["first", "second", "block"])
            .load(&load_ctx, &resolver)
            .await
    });

    blocker.entered.notified().await;
    load.abort();
    assert!(matches!(load.await, Err(ref error) if error.is_cancelled()));

    second.wait().await;
    let deadline = Instant::now() + Duration::from_secs(2);
    while input_drops.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "last Input was not dropped");
        tokio::task::yield_now().await;
    }
    assert_eq!(second.observed_order(), 1);
    assert!(
        !ctx.runtime_snapshot()
            .fibers()
            .iter()
            .any(|fiber| fiber.name().contains("PanicInputPlugin")),
        "the panicking member's own terminal Registry barrier completed"
    );

    first.wait().await;
    assert_eq!(
        first.observed_order(),
        2,
        "rollback must attempt the prior member"
    );
    wait_for_root_only(&ctx).await;
    assert_eq!(input_drops.load(Ordering::SeqCst), 1);
    let reports = buffer.snapshot();
    assert_eq!(
        reports.len(),
        1,
        "one contained destructor report reaches the exporter"
    );
    assert_eq!(reports[0].level(), Level::Warn);
    assert_eq!(reports[0].channel(), "handoff::-panic-input-plugin");
    assert_eq!(
        reports[0].text(),
        "cordis: Loader rollback member destruction panicked: last input reference dropped during loader rollback"
    );
}

/// Loader awaits core spawn inline (issue #232, #224 U1). A load future held
/// unpolled after an entry's spawn commit therefore keeps that creation where it
/// is, as a held `Context::spawn` future does, and starts no rollback. Resuming
/// it finishes the load; dropping it instead hands the in-progress creation and
/// the rollback to framework completion (the abandonment tests above).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn held_load_future_advances_its_in_progress_spawn_only_as_it_is_polled() {
    let ctx = Context::new();
    let first = CleanupProbe::new(CleanupMode::Ok, Arc::new(AtomicUsize::new(0)));
    let blocker = BlockProbe::new();
    let plan = plan(&["first", "block"]);
    let resolver_probe = first.clone();
    let resolver_blocker = blocker.clone();
    let resolver =
        move |request: PluginRequest<'_>| -> Result<Option<PreparedPlugin>, ResolverError> {
            if request.resolve_key() == "block" {
                return Ok(Some(PreparedPlugin::from_input(
                    BlockingPlugin(resolver_blocker.clone()),
                    (),
                )));
            }
            Ok(Some(PreparedPlugin::from_input(
                CleanupPlugin(resolver_probe.clone()),
                (),
            )))
        };

    let mut load = Box::pin(plan.load(&ctx, &resolver));
    tokio::select! {
        _ = &mut load => panic!("load cannot finish while its second apply is parked"),
        () = blocker.entered.notified() => {}
    }

    // `load` is now alive and unpolled, like the unfinished half of a select.
    blocker.release.notify_one();
    let settle_window = Instant::now() + Duration::from_millis(100);
    while Instant::now() < settle_window {
        tokio::task::yield_now().await;
    }
    let states: Vec<FiberState> = ctx
        .runtime_snapshot()
        .fibers()
        .iter()
        .filter(|fiber| fiber.role() == FiberRole::Ordinary)
        .map(|fiber| fiber.state())
        .collect();
    assert_eq!(states.len(), 2, "both entries committed: {states:?}");
    assert!(
        states.contains(&FiberState::Loading),
        "the held load future does not advance its in-progress spawn: {states:?}"
    );
    assert_eq!(
        first.observed_order(),
        0,
        "a held load future starts no rollback"
    );

    let outcome = tokio::select! {
        outcome = &mut load => outcome,
        () = async {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                tokio::task::yield_now().await;
            }
        } => panic!("resumed load did not finish"),
    };
    assert!(
        outcome
            .entries()
            .iter()
            .all(|entry| matches!(entry, EntryOutcome::Spawned { .. })),
        "resuming the held load delivers every entry"
    );
    for fiber_handle in outcome.fiber_handles() {
        fiber_handle.dispose().await.unwrap();
    }
    first.wait().await;
    wait_for_root_only(&ctx).await;
}

#[tokio::test]
async fn ordinary_entry_failure_keeps_prior_success_caller_owned_and_load_remains_partial() {
    let ctx = Context::new();
    let order = Arc::new(AtomicUsize::new(0));
    let first = CleanupProbe::new(CleanupMode::Ok, order);
    let plan = plan(&["first", "failed"]);
    let probe_for_resolver = first.clone();
    let resolver = move |request: PluginRequest<'_>| {
        if request.resolve_key() == "failed" {
            Err(ResolverError)
        } else {
            Ok(Some(PreparedPlugin::from_input(
                CleanupPlugin(probe_for_resolver.clone()),
                (),
            )))
        }
    };

    let outcome = plan.load(&ctx, &resolver).await;
    assert!(matches!(outcome.entries()[0], EntryOutcome::Spawned { .. }));
    assert!(matches!(
        outcome.entries()[1],
        EntryOutcome::Failed {
            failure: LoaderFailure::Resolver(_),
            ..
        }
    ));
    assert_eq!(
        first.observed_order(),
        0,
        "entry failure is not global rollback"
    );
    assert_eq!(ctx.runtime_snapshot().fibers().len(), 2);

    let fiber_handle = outcome.fiber_handles().next().unwrap().clone();
    fiber_handle.dispose().await.unwrap();
    first.wait().await;
    wait_for_root_only(&ctx).await;
}

#[tokio::test]
async fn delivered_outcome_drop_is_inert_and_caller_retains_fiber_handle_ownership() {
    let ctx = Context::new();
    let order = Arc::new(AtomicUsize::new(0));
    let first = CleanupProbe::new(CleanupMode::Ok, order);
    let plan = plan(&["first"]);
    let probe_for_resolver = first.clone();
    let resolver = move |_request: PluginRequest<'_>| {
        Ok::<_, ResolverError>(Some(PreparedPlugin::from_input(
            CleanupPlugin(probe_for_resolver.clone()),
            (),
        )))
    };

    let outcome = plan.load(&ctx, &resolver).await;
    let fiber_handle = outcome.fiber_handles().next().unwrap().clone();
    let id = fiber_handle.id();
    drop(outcome);

    assert_eq!(first.observed_order(), 0, "delivered outcome Drop is inert");
    assert!(
        ctx.runtime_snapshot()
            .fibers()
            .iter()
            .any(|fiber| fiber.id() == &id),
        "caller-owned FiberHandle remains resident after delivered outcome Drop"
    );

    fiber_handle.dispose().await.unwrap();
    first.wait().await;
    wait_for_root_only(&ctx).await;
}
