//! Issue 41 evidence for the flat Runtime current-state projection.

use cordis_core::lifecycle::FiberRole;
use cordis_core::{Context, FiberState, InjectSpec, Plugin, PreparedPlugin, Service};
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug)]
struct Counter;
impl Service for Counter {
    const NAME: &'static str = "issue41-counter";
}

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
    async fn apply(&self, _ctx: Context, _prepared: &()) -> Result<(), Infallible> {
        Ok(())
    }
}

struct Missing;
impl Plugin for Missing {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn inject(&self) -> InjectSpec {
        InjectSpec::none().require(Counter::NAME)
    }
    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, _ctx: Context, _prepared: &()) -> Result<(), Infallible> {
        Ok(())
    }
}

struct LoadingPublisher {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    published: Arc<AtomicBool>,
}
impl Plugin for LoadingPublisher {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), Infallible> {
        let _publication = ctx.provide(Arc::new(Counter)).unwrap();
        self.published.store(true, Ordering::SeqCst);
        self.entered.notify_one();
        self.release.notified().await;
        Ok(())
    }
}

#[test]
fn empty_runtime_snapshot_is_exactly_the_permanent_root() {
    let ctx = Context::new();
    let snapshot = ctx.runtime_snapshot();

    assert_eq!(snapshot.fibers().len(), 1);
    let root = &snapshot.fibers()[0];
    assert_eq!(root.role(), FiberRole::Root);
    assert_eq!(root.state(), FiberState::Active);
    assert!(root.missing_services().is_empty());
    assert!(snapshot.services().is_empty());
}

#[tokio::test]
async fn snapshot_is_flat_current_residency_with_per_record_semantics() {
    let ctx = Context::new();
    let active = ctx.spawn(prepared(Plain)).await.unwrap();
    let pending = ctx.spawn(prepared(Missing)).await.unwrap();

    let snapshot = ctx.runtime_snapshot();
    assert_eq!(
        snapshot.fibers().len(),
        3,
        "root plus two resident ordinary Fibers"
    );
    assert_eq!(
        snapshot
            .fibers()
            .iter()
            .filter(|r| r.role() == FiberRole::Root)
            .count(),
        1
    );

    let active_record = snapshot
        .fibers()
        .iter()
        .find(|r| r.id() == &active.id())
        .unwrap();
    assert_eq!(active_record.role(), FiberRole::Ordinary);
    assert_eq!(active_record.state(), FiberState::Active);
    assert!(active_record.missing_services().is_empty());

    let pending_record = snapshot
        .fibers()
        .iter()
        .find(|r| r.id() == &pending.id())
        .unwrap();
    assert_eq!(pending_record.state(), FiberState::Pending);
    assert_eq!(
        pending_record.missing_services(),
        &[Counter::NAME.to_owned()]
    );
}

#[tokio::test]
async fn loading_publication_is_current_but_invisible_then_becomes_visible() {
    let ctx = Context::new();
    let realm = ctx.new_service_realm();
    let isolated = ctx
        .with_service_realms([(Counter::NAME, realm.clone())])
        .unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let published = Arc::new(AtomicBool::new(false));

    let spawn = tokio::spawn({
        let isolated = isolated.clone();
        let entered = entered.clone();
        let release = release.clone();
        let published = published.clone();
        async move {
            isolated
                .spawn(prepared(LoadingPublisher {
                    entered,
                    release,
                    published,
                }))
                .await
        }
    });
    entered.notified().await;
    assert!(published.load(Ordering::SeqCst));

    let loading = ctx.runtime_snapshot();
    let service = loading
        .services()
        .iter()
        .find(|r| r.service() == Counter::NAME)
        .unwrap();
    assert_eq!(service.realm(), &realm);
    assert!(
        !service.visible(),
        "Loading occupation is current but lookup-invisible"
    );
    let provider = loading
        .fibers()
        .iter()
        .find(|r| r.id() == service.provider())
        .unwrap();
    assert_eq!(provider.state(), FiberState::Loading);

    release.notify_one();
    let fiber_handle = spawn.await.unwrap().unwrap();
    let active = ctx.runtime_snapshot();
    let service_after = active
        .services()
        .iter()
        .find(|r| r.service() == Counter::NAME)
        .unwrap();
    assert_eq!(
        service_after.id(),
        service.id(),
        "visibility does not replace occurrence identity"
    );
    assert!(service_after.visible());
    assert_eq!(service_after.provider(), &fiber_handle.id());
}

struct BlockingServiceCleanup {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}
impl Plugin for BlockingServiceCleanup {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), Infallible> {
        let _publication = ctx.provide(Arc::new(Counter)).unwrap();
        let entered = self.entered.clone();
        let release = self.release.clone();
        ctx.effect(move || async move {
            entered.notify_one();
            release.notified().await;
        })
        .unwrap();
        Ok(())
    }
}

#[tokio::test]
async fn closed_generation_physical_service_row_is_not_current_snapshot_state() {
    let ctx = Context::new();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let fiber_handle = ctx
        .spawn(prepared(BlockingServiceCleanup {
            entered: entered.clone(),
            release: release.clone(),
        }))
        .await
        .unwrap();
    assert_eq!(ctx.runtime_snapshot().services().len(), 1);

    let disposing = tokio::spawn({
        let fiber_handle = fiber_handle.clone();
        async move { fiber_handle.dispose().await }
    });
    entered.notified().await;
    assert_eq!(fiber_handle.state(), FiberState::Unloading);

    let during_cleanup = ctx.runtime_snapshot();
    assert!(
        during_cleanup.services().is_empty(),
        "a closed-generation physical row is stale semantic state"
    );
    let fiber = during_cleanup
        .fibers()
        .iter()
        .find(|r| r.id() == &fiber_handle.id())
        .unwrap();
    assert_eq!(
        fiber.state(),
        FiberState::Unloading,
        "residency outlives generation cleanup"
    );

    release.notify_one();
    disposing.await.unwrap().unwrap();
}

#[tokio::test]
async fn later_snapshot_recovers_current_state_after_an_unobserved_gap_only() {
    let ctx = Context::new();
    let before = ctx.runtime_snapshot();
    let fiber_handle = ctx.spawn(prepared(Plain)).await.unwrap();
    let _publication = ctx.provide(Arc::new(Counter)).unwrap();

    let recovered = ctx.runtime_snapshot();
    assert!(
        recovered
            .fibers()
            .iter()
            .any(|r| r.id() == &fiber_handle.id())
    );
    assert_eq!(recovered.services().len(), 1);
    assert_eq!(
        before.fibers().len(),
        1,
        "an older immutable snapshot is not retroactively mutated"
    );

    fiber_handle.dispose().await.unwrap();
    let latest = ctx.runtime_snapshot();
    assert_eq!(
        latest.fibers().len(),
        1,
        "later snapshots expose current residency, not transition history"
    );
    assert_eq!(
        latest.services().len(),
        1,
        "root-owned publication remains current independently of the disposed ordinary Fiber"
    );
}
