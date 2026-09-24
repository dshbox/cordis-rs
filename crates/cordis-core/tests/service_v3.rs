//! Issue 24/25 evidence for exact Service-slot publication, visibility, and control.

mod common;

use cordis_core::service::{
    ServiceControlError, ServiceLookupError, ServicePublication, ServicePublishError,
};
use cordis_core::{
    Context, FiberState, InjectSpec, Plugin, PreparedChange, PreparedPlugin, Service,
};
use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Debug)]
struct Counter(u32);
impl Service for Counter {
    const NAME: &'static str = "t24-counter";
}

#[derive(Debug)]
struct SameNameOther;
impl Service for SameNameOther {
    const NAME: &'static str = Counter::NAME;
}

fn prepared<P: Plugin<Config = (), Input = (), PrepareError = Infallible>>(
    plugin: P,
) -> PreparedPlugin {
    PreparedPlugin::from_input(plugin, ())
}

struct WantsCounter {
    applied: Arc<AtomicU32>,
}

impl Plugin for WantsCounter {
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

    fn apply(
        &self,
        ctx: Context,
        _prepared: &(),
    ) -> impl Future<Output = Result<(), Infallible>> + Send {
        let applied = self.applied.clone();
        async move {
            let _ = ctx
                .try_service::<Counter>()
                .expect("dependency is visible at apply");
            applied.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
}

#[tokio::test]
async fn exact_lookup_distinguishes_unavailable_contract_mismatch_and_realms() {
    let root = Context::new();
    let isolated = root.with_isolated_service(Counter::NAME);

    assert_eq!(
        root.try_service::<Counter>().unwrap_err(),
        ServiceLookupError::Unavailable {
            service: Counter::NAME
        }
    );

    let _global = root.provide(Arc::new(Counter(1))).unwrap();
    assert_eq!(root.try_service::<Counter>().unwrap().0, 1);
    assert_eq!(
        isolated.try_service::<Counter>().unwrap_err(),
        ServiceLookupError::Unavailable {
            service: Counter::NAME
        },
        "exact isolated lookup has no fallback to the default realm"
    );
    assert_eq!(
        isolated.try_service::<SameNameOther>().unwrap_err(),
        ServiceLookupError::ContractMismatch {
            service: Counter::NAME
        },
        "the Runtime-wide same-name contract mismatch remains distinguishable even when this realm is empty"
    );

    let _private = isolated.provide(Arc::new(Counter(2))).unwrap();
    assert_eq!(root.try_service::<Counter>().unwrap().0, 1);
    assert_eq!(isolated.try_service::<Counter>().unwrap().0, 2);
}

#[tokio::test]
async fn duplicate_and_contract_mismatch_publication_refuse_without_disturbing_current_occurrence()
{
    let root = Context::new();
    let _publication = root.provide(Arc::new(Counter(1))).unwrap();

    let duplicate = match root.provide(Arc::new(Counter(2))) {
        Err(error) => error,
        Ok(_) => panic!("duplicate publication unexpectedly succeeded"),
    };
    assert_eq!(
        duplicate,
        ServicePublishError::DuplicatePublication {
            service: Counter::NAME
        }
    );

    let isolated = root.with_isolated_service(Counter::NAME);
    let mismatch = match isolated.provide(Arc::new(SameNameOther)) {
        Err(error) => error,
        Ok(_) => panic!("incompatible same-name contract unexpectedly published"),
    };
    assert_eq!(
        mismatch,
        ServicePublishError::ContractMismatch {
            service: Counter::NAME
        }
    );

    assert_eq!(root.try_service::<Counter>().unwrap().0, 1);
    assert_eq!(
        isolated.try_service::<Counter>().unwrap_err(),
        ServiceLookupError::Unavailable {
            service: Counter::NAME
        },
        "refused publication leaves no exact-slot residue"
    );
}

#[tokio::test]
async fn undeclared_active_late_publication_creates_dependency_drift() {
    let root = Context::new();
    let applied = Arc::new(AtomicU32::new(0));
    let dependent = root
        .spawn(prepared(WantsCounter {
            applied: applied.clone(),
        }))
        .await
        .unwrap();
    assert_eq!(dependent.state(), FiberState::Pending);
    assert_eq!(applied.load(Ordering::SeqCst), 0);

    let _publication = root.provide(Arc::new(Counter(1))).unwrap();
    assert_eq!(dependent.ready().await.unwrap(), FiberState::Active);
    assert_eq!(
        applied.load(Ordering::SeqCst),
        1,
        "undeclared late publication creates drift"
    );
    assert_eq!(root.try_service::<Counter>().unwrap().0, 1);
}

#[tokio::test]
async fn off_runtime_visibility_commit_is_driven_by_later_ready() {
    let root = Context::new();
    let applied = Arc::new(AtomicU32::new(0));
    let dependent = root
        .spawn(prepared(WantsCounter {
            applied: applied.clone(),
        }))
        .await
        .unwrap();
    assert_eq!(dependent.state(), FiberState::Pending);

    let publication = std::thread::spawn({
        let root = root.clone();
        move || {
            let first = root.provide(Arc::new(Counter(26))).unwrap();
            first.remove().unwrap();
            root.provide(Arc::new(Counter(27))).unwrap()
        }
    })
    .join()
    .unwrap();

    assert_eq!(
        dependent.state(),
        FiberState::Pending,
        "no executor was available to run the kicks"
    );
    assert_eq!(dependent.ready().await.unwrap(), FiberState::Active);
    assert_eq!(
        applied.load(Ordering::SeqCst),
        1,
        "three off-runtime visibility commits coalesce directly to the latest target"
    );
    assert_eq!(root.try_service::<Counter>().unwrap().0, 27);
    drop(publication);
}

#[tokio::test]
async fn multi_slot_visibility_batch_rechecks_each_dependent_once() {
    struct Gauge;
    impl Service for Gauge {
        const NAME: &'static str = "t27-gauge";
    }

    struct WantsBoth(Arc<AtomicU32>);
    impl Plugin for WantsBoth {
        type Config = ();
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = Infallible;

        fn inject(&self) -> InjectSpec {
            InjectSpec::none()
                .require(Counter::NAME)
                .require(Gauge::NAME)
        }

        fn prepare(&self, (): ()) -> Result<(), Infallible> {
            Ok(())
        }

        async fn apply(&self, _ctx: Context, _prepared: &()) -> Result<(), Infallible> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct ProvidesBoth;
    impl Plugin for ProvidesBoth {
        type Config = ();
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = Infallible;

        fn prepare(&self, (): ()) -> Result<(), Infallible> {
            Ok(())
        }

        async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), Infallible> {
            let _counter = ctx.provide(Arc::new(Counter(27))).unwrap();
            let _gauge = ctx.provide(Arc::new(Gauge)).unwrap();
            Ok(())
        }
    }

    let root = Context::new();
    let applied = Arc::new(AtomicU32::new(0));
    let dependent = root
        .spawn(prepared(WantsBoth(applied.clone())))
        .await
        .unwrap();
    assert_eq!(dependent.state(), FiberState::Pending);

    let _provider = root.spawn(prepared(ProvidesBoth)).await.unwrap();
    assert_eq!(dependent.ready().await.unwrap(), FiberState::Active);
    assert_eq!(
        applied.load(Ordering::SeqCst),
        1,
        "the two-slot Active visibility batch must deduplicate one dependent Fiber"
    );
}

#[tokio::test]
async fn realm_mapping_derivation_does_not_create_dependency_drift() {
    let root = Context::new();
    let _publication = root.provide(Arc::new(Counter(1))).unwrap();
    let applied = Arc::new(AtomicU32::new(0));
    let dependent = root
        .spawn(prepared(WantsCounter {
            applied: applied.clone(),
        }))
        .await
        .unwrap();
    assert_eq!(dependent.ready().await.unwrap(), FiberState::Active);
    assert_eq!(applied.load(Ordering::SeqCst), 1);

    let _derived = root.with_isolated_service(Counter::NAME);
    assert_eq!(dependent.ready().await.unwrap(), FiberState::Active);
    assert_eq!(
        applied.load(Ordering::SeqCst),
        1,
        "realm-mapping derivation changes no effective publication visibility"
    );
}

struct LoadingProvider {
    installed: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

impl Plugin for LoadingProvider {
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
        let installed = self.installed.clone();
        let release = self.release.clone();
        async move {
            let _publication = ctx.provide(Arc::new(Counter(7))).unwrap();
            installed.notify_one();
            release.notified().await;
            Ok(())
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loading_occupation_is_invisible_until_active_then_visibility_drifts() {
    let root = Context::new();
    let installed = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let provider_root = root.clone();
    let provider_installed = installed.clone();
    let provider_release = release.clone();
    let provider_task = tokio::spawn(async move {
        provider_root
            .spawn(prepared(LoadingProvider {
                installed: provider_installed,
                release: provider_release,
            }))
            .await
            .unwrap()
    });

    installed.notified().await;
    assert_eq!(
        root.try_service::<Counter>().unwrap_err(),
        ServiceLookupError::Unavailable {
            service: Counter::NAME
        },
        "Loading occurrence occupies storage but is invisible"
    );
    assert_eq!(
        match root.provide(Arc::new(Counter(8))) {
            Err(error) => error,
            Ok(_) => panic!("Loading occupation failed to reserve the exact slot"),
        },
        ServicePublishError::DuplicatePublication {
            service: Counter::NAME
        },
        "Loading occupation is already the exact slot's eligible current publication"
    );

    let applied = Arc::new(AtomicU32::new(0));
    let dependent = root
        .spawn(prepared(WantsCounter {
            applied: applied.clone(),
        }))
        .await
        .unwrap();
    assert_eq!(dependent.state(), FiberState::Pending);
    assert_eq!(applied.load(Ordering::SeqCst), 0);

    release.notify_one();
    let provider = provider_task.await.unwrap();
    assert_eq!(provider.ready().await.unwrap(), FiberState::Active);
    assert_eq!(dependent.ready().await.unwrap(), FiberState::Active);
    assert_eq!(applied.load(Ordering::SeqCst), 1);
    assert_eq!(root.try_service::<Counter>().unwrap().0, 7);
}

struct DrainingDependent {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

impl Plugin for DrainingDependent {
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

    async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), Infallible> {
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

#[test]
fn service_cleanup_convergence_survives_origin_runtime_shutdown() {
    let origin = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let root = Context::new();
    let provider_started = Arc::new(tokio::sync::Notify::new());
    let provider_release = Arc::new(tokio::sync::Notify::new());
    let dependent_started = Arc::new(tokio::sync::Notify::new());
    let dependent_release = Arc::new(tokio::sync::Notify::new());
    let provider = origin
        .block_on(root.spawn(prepared(ParkingProvider {
            cleanup_started: provider_started.clone(),
            release_cleanup: provider_release.clone(),
        })))
        .unwrap();
    let dependent = origin
        .block_on(root.spawn(prepared(DrainingDependent {
            entered: dependent_started.clone(),
            release: dependent_release.clone(),
        })))
        .unwrap();
    assert_eq!(dependent.state(), FiberState::Active);

    let disposing = provider.clone();
    origin.spawn(async move {
        disposing.dispose().await.unwrap();
    });
    // The dependent pass has the slot and awaits its actual cleanup on the
    // completion runtime. Shutting down now drops its original Tokio task.
    origin.block_on(dependent_started.notified());
    origin.shutdown_background();
    provider_release.notify_one();
    dependent_release.notify_one();

    let observer = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let outcome = observer.block_on(async {
        tokio::time::timeout(std::time::Duration::from_millis(500), dependent.ready()).await
    });
    assert_eq!(
        outcome
            .expect("committed dependent recheck survives origin shutdown")
            .unwrap(),
        FiberState::Pending
    );
    observer.block_on(provider.dispose()).unwrap();
    observer.block_on(dependent.dispose()).unwrap();
}

struct ParkingProvider {
    cleanup_started: Arc<tokio::sync::Notify>,
    release_cleanup: Arc<tokio::sync::Notify>,
}

impl Plugin for ParkingProvider {
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
        let cleanup_started = self.cleanup_started.clone();
        let release_cleanup = self.release_cleanup.clone();
        async move {
            let _publication = ctx.provide(Arc::new(Counter(10))).unwrap();
            let _parking = ctx
                .effect(move || async move {
                    cleanup_started.notify_one();
                    release_cleanup.notified().await;
                })
                .unwrap();
            Ok(())
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_withdraws_before_cleanup_and_stale_cleanup_cannot_remove_replacement() {
    let root = Context::new();
    let applied = Arc::new(AtomicU32::new(0));
    let dependent = root
        .spawn(prepared(WantsCounter {
            applied: applied.clone(),
        }))
        .await
        .unwrap();
    assert_eq!(dependent.state(), FiberState::Pending);

    let cleanup_started = Arc::new(tokio::sync::Notify::new());
    let release_cleanup = Arc::new(tokio::sync::Notify::new());
    let provider = root
        .spawn(prepared(ParkingProvider {
            cleanup_started: cleanup_started.clone(),
            release_cleanup: release_cleanup.clone(),
        }))
        .await
        .unwrap();
    assert_eq!(provider.state(), FiberState::Active);
    assert_eq!(dependent.ready().await.unwrap(), FiberState::Active);
    assert_eq!(applied.load(Ordering::SeqCst), 1);
    assert_eq!(root.try_service::<Counter>().unwrap().0, 10);

    let disposing = tokio::spawn(async move {
        provider.dispose().await.unwrap();
    });
    cleanup_started.notified().await;

    assert_eq!(dependent.ready().await.unwrap(), FiberState::Pending);
    assert_eq!(
        root.try_service::<Counter>().unwrap_err(),
        ServiceLookupError::Unavailable {
            service: Counter::NAME
        },
        "generation close withdraws visibility before its physical publication cleanup"
    );

    let _replacement = root.provide(Arc::new(Counter(20))).unwrap();
    assert_eq!(dependent.ready().await.unwrap(), FiberState::Active);
    assert_eq!(applied.load(Ordering::SeqCst), 2);
    assert_eq!(root.try_service::<Counter>().unwrap().0, 20);

    release_cleanup.notify_one();
    disposing.await.unwrap();
    assert_eq!(
        root.try_service::<Counter>().unwrap().0,
        20,
        "old generation cleanup is exact-occurrence checked and cannot remove the replacement"
    );
    assert_eq!(dependent.ready().await.unwrap(), FiberState::Active);
    assert_eq!(
        applied.load(Ordering::SeqCst),
        2,
        "stale physical cleanup must not create dependency drift"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_creator_after_publication_commit_cleans_the_occurrence() {
    let root = Context::new();
    let installed = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let spawn_root = root.clone();
    let spawn_installed = installed.clone();
    let spawn_release = release.clone();
    let task = tokio::spawn(async move {
        spawn_root
            .spawn(prepared(LoadingProvider {
                installed: spawn_installed,
                release: spawn_release,
            }))
            .await
    });

    installed.notified().await;
    assert_eq!(
        root.try_service::<Counter>().unwrap_err(),
        ServiceLookupError::Unavailable {
            service: Counter::NAME
        },
        "the committed Loading occurrence is still invisible before cancellation"
    );

    task.abort();
    let _ = task.await;
    let mut publication = None;
    for _ in 0..10_000 {
        match root.provide(Arc::new(Counter(55))) {
            Ok(current) => {
                publication = Some(current);
                break;
            }
            Err(ServicePublishError::DuplicatePublication { .. }) => {
                tokio::task::yield_now().await;
            }
            Err(other) => {
                panic!("unexpected replacement refusal after creator cancellation: {other}")
            }
        }
    }
    let _publication =
        publication.expect("creation guard must finish generation-owned Service cleanup");

    assert_eq!(root.try_service::<Counter>().unwrap().0, 55);
}

struct CloseProbe {
    seen: Arc<parking_lot::Mutex<Option<ServicePublishError>>>,
}

impl Plugin for CloseProbe {
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
        let seen = self.seen.clone();
        async move {
            let closed = ctx.clone();
            let _probe = ctx
                .effect_sync(move || {
                    let error = match closed.provide(Arc::new(Counter(99))) {
                        Err(error) => error,
                        Ok(_) => panic!("closed generation published a Service"),
                    };
                    *seen.lock() = Some(error);
                })
                .unwrap();
            Ok(())
        }
    }
}

#[tokio::test]
async fn closing_generation_refuses_publication_without_slot_residue() {
    let root = Context::new();
    let seen = Arc::new(parking_lot::Mutex::new(None));
    let fiber_handle = root
        .spawn(prepared(CloseProbe { seen: seen.clone() }))
        .await
        .unwrap();
    fiber_handle.dispose().await.unwrap();

    assert_eq!(
        seen.lock().take(),
        Some(ServicePublishError::InactiveContext)
    );
    let _root_publication = root
        .provide(Arc::new(Counter(100)))
        .expect("refusal leaves no slot residue");
    assert_eq!(root.try_service::<Counter>().unwrap().0, 100);
}

#[test]
fn direct_lookup_needs_no_inject_membership() {
    let root = Context::new();
    let _publication = root.provide(Arc::new(Counter(42))).unwrap();
    assert_eq!(root.try_service::<Counter>().unwrap().0, 42);
}

#[tokio::test]
async fn exact_publication_set_preserves_target_remove_commits_drift_and_drop_is_inert() {
    let root = Context::new();
    let publication = root.provide(Arc::new(Counter(1))).unwrap();
    let applied = Arc::new(AtomicU32::new(0));
    let dependent = root
        .spawn(prepared(WantsCounter {
            applied: applied.clone(),
        }))
        .await
        .unwrap();
    assert_eq!(dependent.ready().await.unwrap(), FiberState::Active);
    assert_eq!(applied.load(Ordering::SeqCst), 1);

    publication.set(Arc::new(Counter(2))).unwrap();
    assert_eq!(root.try_service::<Counter>().unwrap().0, 2);
    assert_eq!(dependent.ready().await.unwrap(), FiberState::Active);
    assert_eq!(
        applied.load(Ordering::SeqCst),
        1,
        "same-occurrence payload set must not create dependency-target drift"
    );

    publication.remove().unwrap();
    assert_eq!(dependent.ready().await.unwrap(), FiberState::Pending);
    assert_eq!(
        root.try_service::<Counter>().unwrap_err(),
        ServiceLookupError::Unavailable {
            service: Counter::NAME
        }
    );

    let replacement = root.provide(Arc::new(Counter(3))).unwrap();
    assert_eq!(dependent.ready().await.unwrap(), FiberState::Active);
    assert_eq!(applied.load(Ordering::SeqCst), 2);
    drop(replacement);
    assert_eq!(
        root.try_service::<Counter>().unwrap().0,
        3,
        "dropping ServicePublication is inert"
    );
}

struct CapturingProvider {
    publication: Arc<parking_lot::Mutex<Option<ServicePublication<Counter>>>>,
    block_cleanup: bool,
    cleanup_started: Arc<tokio::sync::Notify>,
    release_cleanup: Arc<tokio::sync::Notify>,
}

impl Plugin for CapturingProvider {
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
        let publication = self.publication.clone();
        let block_cleanup = self.block_cleanup;
        let cleanup_started = self.cleanup_started.clone();
        let release_cleanup = self.release_cleanup.clone();
        async move {
            let current = ctx.provide(Arc::new(Counter(10))).unwrap();
            *publication.lock() = Some(current);
            if block_cleanup {
                let _parking = ctx
                    .effect(move || async move {
                        cleanup_started.notify_one();
                        release_cleanup.notified().await;
                    })
                    .unwrap();
            }
            Ok(())
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn terminal_claim_closes_exact_service_mutation_before_unloading() {
    let root = Context::new();
    let publication = Arc::new(parking_lot::Mutex::new(None));
    let provider = root
        .spawn(prepared(CapturingProvider {
            publication: publication.clone(),
            block_cleanup: false,
            cleanup_started: Arc::new(tokio::sync::Notify::new()),
            release_cleanup: Arc::new(tokio::sync::Notify::new()),
        }))
        .await
        .unwrap();
    assert_eq!(provider.state(), FiberState::Active);

    // Poll through the synchronous terminal claim. The detached teardown cannot
    // run on this thread before we check the still-Active publication.
    let dispose = provider.dispose();
    tokio::pin!(dispose);
    assert!(futures::poll!(&mut dispose).is_pending());
    assert_eq!(provider.state(), FiberState::Active);

    let occurrence = publication.lock().take().unwrap();
    assert_eq!(
        occurrence.set(Arc::new(Counter(11))),
        Err(ServiceControlError::MutationClosed {
            service: Counter::NAME,
        })
    );
    assert_eq!(
        occurrence.remove(),
        Err(ServiceControlError::MutationClosed {
            service: Counter::NAME,
        })
    );
    dispose.await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn restart_commit_closes_old_publication_mutation_before_unloading() {
    let root = Context::new();
    let publication = Arc::new(parking_lot::Mutex::new(None));
    let provider = root
        .spawn(prepared(CapturingProvider {
            publication: publication.clone(),
            block_cleanup: false,
            cleanup_started: Arc::new(tokio::sync::Notify::new()),
            release_cleanup: Arc::new(tokio::sync::Notify::new()),
        }))
        .await
        .unwrap();
    let old = publication.lock().take().unwrap();

    let restart = provider.restart();
    tokio::pin!(restart);
    assert!(futures::poll!(&mut restart).is_pending());
    assert_eq!(provider.state(), FiberState::Active);
    assert_eq!(
        old.set(Arc::new(Counter(11))),
        Err(ServiceControlError::MutationClosed {
            service: Counter::NAME,
        })
    );
    restart.await.unwrap();
    provider.dispose().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_remove_wins_exact_cleanup_claim_and_old_generation_cannot_touch_replacement() {
    let root = Context::new();
    let publication = Arc::new(parking_lot::Mutex::new(None));
    let provider = root
        .spawn(prepared(CapturingProvider {
            publication: publication.clone(),
            block_cleanup: false,
            cleanup_started: Arc::new(tokio::sync::Notify::new()),
            release_cleanup: Arc::new(tokio::sync::Notify::new()),
        }))
        .await
        .unwrap();

    publication
        .lock()
        .take()
        .expect("provider handed out its exact publication capability")
        .remove()
        .unwrap();
    let _replacement = root.provide(Arc::new(Counter(20))).unwrap();

    provider.dispose().await.unwrap();
    assert_eq!(
        root.try_service::<Counter>().unwrap().0,
        20,
        "manual remove disarms the generation-owned exact cleanup claim"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generation_close_wins_consuming_remove_before_publication_cleanup_runs() {
    let root = Context::new();
    let publication = Arc::new(parking_lot::Mutex::new(None));
    let cleanup_started = Arc::new(tokio::sync::Notify::new());
    let release_cleanup = Arc::new(tokio::sync::Notify::new());
    let provider = root
        .spawn(prepared(CapturingProvider {
            publication: publication.clone(),
            block_cleanup: true,
            cleanup_started: cleanup_started.clone(),
            release_cleanup: release_cleanup.clone(),
        }))
        .await
        .unwrap();

    let disposing = tokio::spawn(async move {
        provider.dispose().await.unwrap();
    });
    cleanup_started.notified().await;

    let current = publication.lock().take().unwrap();
    assert_eq!(
        current.remove().unwrap_err(),
        ServiceControlError::MutationClosed {
            service: Counter::NAME
        },
        "generation close wins before the exact publication cleanup is claimed"
    );

    release_cleanup.notify_one();
    disposing.await.unwrap();
    let _replacement = root
        .provide(Arc::new(Counter(30)))
        .expect("generation cleanup still owns and removes the closed occurrence");
    assert_eq!(root.try_service::<Counter>().unwrap().0, 30);
}

struct RestartingProvider {
    publications: Arc<parking_lot::Mutex<Vec<ServicePublication<Counter>>>>,
    next: Arc<AtomicU32>,
}

impl Plugin for RestartingProvider {
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
        let publications = self.publications.clone();
        let value = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        async move {
            publications
                .lock()
                .push(ctx.provide(Arc::new(Counter(value))).unwrap());
            Ok(())
        }
    }
}

#[tokio::test]
async fn same_fiber_new_generation_replacement_makes_old_publication_handle_stale() {
    let root = Context::new();
    let publications = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let fiber_handle = root
        .spawn(prepared(RestartingProvider {
            publications: publications.clone(),
            next: Arc::new(AtomicU32::new(0)),
        }))
        .await
        .unwrap();
    let old = publications
        .lock()
        .pop()
        .expect("first generation handed out its publication capability");
    assert_eq!(root.try_service::<Counter>().unwrap().0, 1);

    fiber_handle.restart().await.unwrap();
    assert_eq!(root.try_service::<Counter>().unwrap().0, 2);
    assert_eq!(
        old.set(Arc::new(Counter(99))).unwrap_err(),
        ServiceControlError::StalePublication {
            service: Counter::NAME
        },
        "same Fiber identity does not preserve publication occurrence authority across generations"
    );
    assert_eq!(
        old.remove().unwrap_err(),
        ServiceControlError::StalePublication {
            service: Counter::NAME
        }
    );
    assert_eq!(root.try_service::<Counter>().unwrap().0, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closed_current_reports_mutation_closed_but_replacement_makes_old_handle_stale_first() {
    let root = Context::new();
    let publication = Arc::new(parking_lot::Mutex::new(None));
    let cleanup_started = Arc::new(tokio::sync::Notify::new());
    let release_cleanup = Arc::new(tokio::sync::Notify::new());
    let provider = root
        .spawn(prepared(CapturingProvider {
            publication: publication.clone(),
            block_cleanup: true,
            cleanup_started: cleanup_started.clone(),
            release_cleanup: release_cleanup.clone(),
        }))
        .await
        .unwrap();

    let disposing = tokio::spawn(async move {
        provider.dispose().await.unwrap();
    });
    cleanup_started.notified().await;

    assert_eq!(
        publication
            .lock()
            .as_ref()
            .unwrap()
            .set(Arc::new(Counter(11)))
            .unwrap_err(),
        ServiceControlError::MutationClosed {
            service: Counter::NAME
        },
        "a still-current occurrence is checked for closure after exact identity"
    );

    let _replacement = root.provide(Arc::new(Counter(20))).unwrap();
    let old = publication.lock().take().unwrap();
    assert_eq!(
        old.set(Arc::new(Counter(12))).unwrap_err(),
        ServiceControlError::StalePublication {
            service: Counter::NAME
        },
        "stale identity must win over the old generation's closed state"
    );
    assert_eq!(
        old.remove().unwrap_err(),
        ServiceControlError::StalePublication {
            service: Counter::NAME
        },
        "consuming remove uses the same stale-before-closed priority"
    );

    release_cleanup.notify_one();
    disposing.await.unwrap();
    assert_eq!(
        root.try_service::<Counter>().unwrap().0,
        20,
        "stale generation cleanup cannot affect the replacement"
    );
}

struct ReentrantValue {
    ctx: Context,
    drops: Arc<AtomicU32>,
}
impl Service for ReentrantValue {
    const NAME: &'static str = "t25-reentrant-value";
}
impl Drop for ReentrantValue {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
        let _ = self.ctx.try_service::<ReentrantValue>();
    }
}

#[test]
fn successful_set_and_remove_destroy_outgoing_values_outside_service_synchronization() {
    let root = Context::new();
    let drops = Arc::new(AtomicU32::new(0));
    let publication = root
        .provide(Arc::new(ReentrantValue {
            ctx: root.clone(),
            drops: drops.clone(),
        }))
        .unwrap();

    let root_for_set = root.clone();
    let drops_for_set = drops.clone();
    let publication = common::deadlock_watchdog(
        "ServicePublication::set deadlocked while dropping the outgoing value",
        move || {
            publication
                .set(Arc::new(ReentrantValue {
                    ctx: root_for_set,
                    drops: drops_for_set,
                }))
                .unwrap();
            publication
        },
    );
    assert_eq!(drops.load(Ordering::SeqCst), 1);

    common::deadlock_watchdog(
        "ServicePublication::remove deadlocked while dropping the withdrawn value",
        move || publication.remove().unwrap(),
    );
    assert_eq!(drops.load(Ordering::SeqCst), 2);
}

struct ReentrantCapturingProvider {
    publication: Arc<parking_lot::Mutex<Option<ServicePublication<ReentrantValue>>>>,
    cleanup_started: Arc<tokio::sync::Notify>,
    release_cleanup: Arc<tokio::sync::Notify>,
    drops: Arc<AtomicU32>,
}

impl Plugin for ReentrantCapturingProvider {
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
        let publication = self.publication.clone();
        let cleanup_started = self.cleanup_started.clone();
        let release_cleanup = self.release_cleanup.clone();
        let drops = self.drops.clone();
        async move {
            let current = ctx
                .provide(Arc::new(ReentrantValue {
                    ctx: ctx.clone(),
                    drops,
                }))
                .unwrap();
            *publication.lock() = Some(current);
            let _parking = ctx
                .effect(move || async move {
                    cleanup_started.notify_one();
                    release_cleanup.notified().await;
                })
                .unwrap();
            Ok(())
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closed_and_stale_set_refusals_drop_rejected_values_outside_synchronization() {
    let root = Context::new();
    let publication = Arc::new(parking_lot::Mutex::new(None));
    let cleanup_started = Arc::new(tokio::sync::Notify::new());
    let release_cleanup = Arc::new(tokio::sync::Notify::new());
    let drops = Arc::new(AtomicU32::new(0));
    let provider = root
        .spawn(prepared(ReentrantCapturingProvider {
            publication: publication.clone(),
            cleanup_started: cleanup_started.clone(),
            release_cleanup: release_cleanup.clone(),
            drops: drops.clone(),
        }))
        .await
        .unwrap();

    let disposing = tokio::spawn(async move {
        provider.dispose().await.unwrap();
    });
    cleanup_started.notified().await;

    let closed_handle = publication.clone();
    let root_for_closed = root.clone();
    let drops_for_closed = drops.clone();
    common::deadlock_watchdog(
        "MutationClosed set dropped its rejected value under Service synchronization",
        move || {
            assert_eq!(
                closed_handle
                    .lock()
                    .as_ref()
                    .unwrap()
                    .set(Arc::new(ReentrantValue {
                        ctx: root_for_closed,
                        drops: drops_for_closed,
                    }))
                    .unwrap_err(),
                ServiceControlError::MutationClosed {
                    service: ReentrantValue::NAME
                }
            );
        },
    );

    let replacement = root
        .provide(Arc::new(ReentrantValue {
            ctx: root.clone(),
            drops: drops.clone(),
        }))
        .unwrap();
    let stale = publication.lock().take().unwrap();
    let root_for_stale = root.clone();
    let drops_for_stale = drops.clone();
    let stale = common::deadlock_watchdog(
        "StalePublication set dropped its rejected value under Service synchronization",
        move || {
            assert_eq!(
                stale
                    .set(Arc::new(ReentrantValue {
                        ctx: root_for_stale,
                        drops: drops_for_stale,
                    }))
                    .unwrap_err(),
                ServiceControlError::StalePublication {
                    service: ReentrantValue::NAME
                }
            );
            stale
        },
    );
    assert_eq!(
        stale.remove().unwrap_err(),
        ServiceControlError::StalePublication {
            service: ReentrantValue::NAME
        }
    );

    release_cleanup.notify_one();
    disposing.await.unwrap();
    drop(replacement);
    assert!(
        drops.load(Ordering::SeqCst) >= 3,
        "original and rejected values all drop without deadlock; the replacement remains slot-owned"
    );
}

#[test]
fn exact_publication_control_is_runtime_independent() {
    let root = Context::new();
    let publication = root.provide(Arc::new(Counter(1))).unwrap();
    std::thread::spawn(move || {
        publication
            .set(Arc::new(Counter(2)))
            .expect("set is synchronous and runtime-independent");
        publication
            .remove()
            .expect("remove is synchronous and runtime-independent");
    })
    .join()
    .unwrap();
    assert_eq!(
        root.try_service::<Counter>().unwrap_err(),
        ServiceLookupError::Unavailable {
            service: Counter::NAME
        }
    );
}

#[tokio::test]
async fn fixed_exact_dependency_edges_survive_restart_and_update() {
    let root = Context::new();
    let isolated = root.with_isolated_service(Counter::NAME);
    let applied = Arc::new(AtomicU32::new(0));
    let dependent = isolated
        .spawn(prepared(WantsCounter {
            applied: applied.clone(),
        }))
        .await
        .unwrap();
    assert_eq!(dependent.state(), FiberState::Pending);
    assert_eq!(dependent.pending_missing(), vec![Counter::NAME.to_owned()]);

    let _global = root.provide(Arc::new(Counter(1))).unwrap();
    dependent.restart().await.unwrap();
    assert_eq!(
        dependent.state(),
        FiberState::Pending,
        "restart must not re-resolve the isolated edge"
    );
    let _ = dependent
        .update(PreparedChange::from_input::<WantsCounter>(()))
        .await
        .unwrap();
    assert_eq!(
        dependent.state(),
        FiberState::Pending,
        "same-Fiber update must preserve the era-local edge"
    );
    assert_eq!(applied.load(Ordering::SeqCst), 0);

    let _private = isolated.provide(Arc::new(Counter(2))).unwrap();
    assert_eq!(dependent.ready().await.unwrap(), FiberState::Active);
    assert_eq!(applied.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn pending_missing_is_the_normalized_missing_projection() {
    struct DuplicateRequirement;
    impl Plugin for DuplicateRequirement {
        type Config = ();
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = Infallible;
        fn inject(&self) -> InjectSpec {
            InjectSpec::none()
                .require(Counter::NAME)
                .require(Counter::NAME)
        }
        fn prepare(&self, (): ()) -> Result<(), Infallible> {
            Ok(())
        }
        fn apply(
            &self,
            _ctx: Context,
            _prepared: &(),
        ) -> impl Future<Output = Result<(), Infallible>> + Send {
            std::future::ready(Ok(()))
        }
    }

    let root = Context::new();
    let isolated = root.with_isolated_service(Counter::NAME);
    let _global = root.provide(Arc::new(Counter(1))).unwrap();
    let dependent = isolated
        .spawn(PreparedPlugin::from_input(DuplicateRequirement, ()))
        .await
        .unwrap();
    assert_eq!(dependent.state(), FiberState::Pending);
    assert_eq!(dependent.pending_missing(), vec![Counter::NAME.to_owned()]);

    let _private = isolated.provide(Arc::new(Counter(2))).unwrap();
    assert_eq!(dependent.ready().await.unwrap(), FiberState::Active);
    assert!(
        dependent.pending_missing().is_empty(),
        "Active has no pending diagnostic"
    );
    dependent.dispose().await.unwrap();
    assert!(
        dependent.pending_missing().is_empty(),
        "Disposed has no pending diagnostic"
    );
}

#[tokio::test]
async fn failed_target_parking_uses_exact_publication_and_explicit_restart_bypasses_it() {
    struct FailsAfterFirst {
        applies: Arc<AtomicU32>,
    }
    impl Plugin for FailsAfterFirst {
        type Config = ();
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = std::io::Error;
        fn inject(&self) -> InjectSpec {
            InjectSpec::none().require(Counter::NAME)
        }
        fn prepare(&self, (): ()) -> Result<(), Infallible> {
            Ok(())
        }
        fn apply(
            &self,
            _ctx: Context,
            _prepared: &(),
        ) -> impl Future<Output = Result<(), std::io::Error>> + Send {
            let turn = self.applies.fetch_add(1, Ordering::SeqCst) + 1;
            std::future::ready(if turn == 1 {
                Ok(())
            } else {
                Err(std::io::Error::other("parked failure"))
            })
        }
    }

    let root = Context::new();
    let first = root.provide(Arc::new(Counter(1))).unwrap();
    let applies = Arc::new(AtomicU32::new(0));
    let dependent = root
        .spawn(PreparedPlugin::from_input(
            FailsAfterFirst {
                applies: applies.clone(),
            },
            (),
        ))
        .await
        .unwrap();
    assert_eq!(applies.load(Ordering::SeqCst), 1);

    first.remove().unwrap();
    assert_eq!(dependent.ready().await.unwrap(), FiberState::Pending);
    let second = root.provide(Arc::new(Counter(2))).unwrap();
    assert!(dependent.ready().await.is_err());
    assert_eq!(dependent.state(), FiberState::Failed);
    assert_eq!(applies.load(Ordering::SeqCst), 2);

    second.set(Arc::new(Counter(3))).unwrap();
    assert!(dependent.ready().await.is_err());
    assert_eq!(
        applies.load(Ordering::SeqCst),
        2,
        "same occurrence set is the same failed SemanticTarget"
    );

    assert!(dependent.restart().await.is_err());
    assert_eq!(
        applies.load(Ordering::SeqCst),
        3,
        "explicit restart may retry the same failed target"
    );

    second.remove().unwrap();
    assert_eq!(dependent.ready().await.unwrap(), FiberState::Pending);
    let _third = root.provide(Arc::new(Counter(4))).unwrap();
    assert!(dependent.ready().await.is_err());
    assert_eq!(
        applies.load(Ordering::SeqCst),
        4,
        "remove/re-provide creates a new exact-publication assignment"
    );
}
