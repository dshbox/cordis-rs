//! Public Context/ServiceRealm contract for the v3 root and isolate axes.

use cordis_core::event::observer_sync;
use cordis_core::service::RealmMappingError;
use cordis_core::{
    ConfigurableService, Context, Event, Plugin, PreparedPlugin, Routing, Scope, Service,
    ServiceRealm,
};
use parking_lot::Mutex;
use std::collections::HashSet;
use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counter;

impl Service for Counter {
    const NAME: &'static str = "counter";
}

struct Other;

impl Service for Other {
    const NAME: &'static str = "other";
}

fn assert_frozen_realm_traits<T: std::fmt::Debug + Clone + Eq + std::hash::Hash>() {}

fn mapping_error(result: Result<Context, RealmMappingError>) -> RealmMappingError {
    match result {
        Ok(_) => panic!("realm mapping unexpectedly succeeded"),
        Err(error) => error,
    }
}

#[test]
fn realms_are_opaque_runtime_local_placement_identities() {
    assert_frozen_realm_traits::<ServiceRealm>();

    let runtime_a = Context::new();
    let runtime_b = Context::new();
    let realm = runtime_a.new_service_realm();

    assert_eq!(realm, realm.clone(), "clone preserves exact realm identity");
    assert_ne!(realm, runtime_a.new_service_realm());
    assert_ne!(realm, runtime_b.new_service_realm());

    let _ = runtime_a.provide(Arc::new(Counter)).unwrap();
    assert!(runtime_a.try_service::<Counter>().is_ok());
    assert!(runtime_b.try_service::<Counter>().is_err());

    let mut set = HashSet::new();
    set.insert(realm.clone());
    assert!(set.contains(&realm));
}

#[test]
fn explicit_equal_realms_join_only_the_mapped_service_slot() {
    let root = Context::new();
    let realm = root.new_service_realm();
    let left = root
        .with_service_realms([(Counter::NAME, realm.clone())])
        .unwrap();
    let right = root.with_service_realms([(Counter::NAME, realm)]).unwrap();

    let _ = left.provide(Arc::new(Counter)).unwrap();
    let cloned = left.clone();
    assert!(cloned.try_service::<Counter>().is_ok());
    assert!(right.try_service::<Counter>().is_ok());
    assert!(root.try_service::<Counter>().is_err());

    let _ = root.provide(Arc::new(Other)).unwrap();
    assert!(left.try_service::<Other>().is_ok());
    assert!(right.try_service::<Other>().is_ok());
}

#[test]
fn fresh_private_derivations_never_rendezvous() {
    let root = Context::new();
    let left = root.with_isolated_service(Counter::NAME);
    let right = root.with_isolated_service(Counter::NAME);

    let _ = left.provide(Arc::new(Counter)).unwrap();
    let _ = right.provide(Arc::new(Counter)).unwrap();
    assert!(left.try_service::<Counter>().is_ok());
    assert!(right.try_service::<Counter>().is_ok());
    assert!(root.try_service::<Counter>().is_err());
}

#[test]
fn realm_batch_rejects_duplicates_before_foreign_realms() {
    let runtime = Context::new();
    let local = runtime.new_service_realm();
    let foreign = Context::new().new_service_realm();

    let error = mapping_error(
        runtime.with_service_realms([(Counter::NAME, foreign), (Counter::NAME, local.clone())]),
    );
    assert!(matches!(
        error,
        RealmMappingError::DuplicateService { service } if service == Counter::NAME
    ));

    let error = mapping_error(
        runtime.with_service_realms([(Other::NAME, Context::new().new_service_realm())]),
    );
    assert!(matches!(
        error,
        RealmMappingError::ForeignRealm { service } if service == Other::NAME
    ));

    // A mixed batch fails atomically: the error names the foreign row, and
    // an implementation that installed the valid local prefix before
    // reporting the foreign row would be observable below.
    let error = mapping_error(runtime.with_service_realms([
        (Counter::NAME, local.clone()),
        (Other::NAME, Context::new().new_service_realm()),
    ]));
    assert!(matches!(
        error,
        RealmMappingError::ForeignRealm { service } if service == Other::NAME
    ));

    // A rejected derivation cannot mutate the source Context or consume a
    // realm — including the mixed batch's valid local prefix.
    let mapped = runtime
        .with_service_realms([(Counter::NAME, local)])
        .unwrap();
    let _ = runtime.provide(Arc::new(Counter)).unwrap();
    let _ = runtime.provide(Arc::new(Other)).unwrap();
    assert!(
        mapped.try_service::<Counter>().is_err(),
        "the rejected Counter→local mapping was never installed"
    );
    assert!(
        runtime.try_service::<Other>().is_ok(),
        "Other's default-realm slot is intact after the rejected batch"
    );
}

#[test]
fn root_stays_in_the_runtime_and_resets_isolation() {
    let runtime = Context::new();
    let isolated = runtime.with_isolated_service(Counter::NAME);
    let reset = isolated.root();

    let _ = reset.provide(Arc::new(Counter)).unwrap();
    assert!(runtime.try_service::<Counter>().is_ok());
    assert!(isolated.try_service::<Counter>().is_err());
}

struct Ping;

impl Event for Ping {
    const NAME: &'static str = "realm-axis-ping";
    type Args = ();
    type Output = ();
}

#[tokio::test]
async fn isolate_derivation_preserves_event_reachability() {
    let root = Context::new().with_intercept::<Layered>("outer".to_owned());
    let isolated = root.with_isolated_service(Counter::NAME);
    let mapped = root
        .with_service_realms([(Other::NAME, root.new_service_realm())])
        .unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let listener_hits = hits.clone();
    let _listener = isolated
        .on::<Ping, _>(observer_sync(move |_, ()| -> Result<(), Infallible> {
            listener_hits.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }))
        .unwrap();
    let listener_hits = hits.clone();
    let _mapped_listener = mapped
        .on::<Ping, _>(observer_sync(move |_, ()| -> Result<(), Infallible> {
            listener_hits.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }))
        .unwrap();

    root.emit::<Ping>(Routing::Scoped(root.scope()), ())
        .await
        .unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    assert_eq!(
        isolated.resolve_config::<Layered>(None, None).unwrap(),
        ["outer"]
    );
    assert_eq!(
        mapped.resolve_config::<Layered>(None, None).unwrap(),
        ["outer"]
    );
}

#[tokio::test]
async fn isolate_derivation_preserves_fiber_attribution() {
    // CX-03 / ADR 0032: an isolate derivation changes only Service
    // placement. Deriving inside a Plugin's apply and registering an
    // effect through the derived view discriminates Fiber attribution:
    // a mutant that re-attributes the derived Context to the root Fiber
    // would never run this cleanup when the FiberHandle is disposed.
    struct DerivesInside {
        cleaned: Arc<AtomicUsize>,
    }

    impl Plugin for DerivesInside {
        type Config = ();
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = Infallible;

        fn prepare(&self, _config: ()) -> Result<(), Infallible> {
            Ok(())
        }

        fn apply(
            &self,
            ctx: Context,
            _prepared: &(),
        ) -> impl Future<Output = Result<(), Infallible>> + Send {
            let derived = ctx.with_isolated_service(Counter::NAME);
            // the derivation really isolates: the root's provider is
            // visible through the applying view, not through the derived one
            assert!(ctx.try_service::<Counter>().is_ok());
            assert!(derived.try_service::<Counter>().is_err());
            let cleaned = self.cleaned.clone();
            derived
                .effect_sync(move || {
                    cleaned.fetch_add(1, Ordering::SeqCst);
                })
                .expect("the applying fiber is live");
            std::future::ready(Ok(()))
        }
    }

    let runtime = Context::new();
    let _ = runtime.provide(Arc::new(Counter)).unwrap();
    let cleaned = Arc::new(AtomicUsize::new(0));
    let fiber_handle = runtime
        .spawn(PreparedPlugin::from_input(
            DerivesInside {
                cleaned: cleaned.clone(),
            },
            (),
        ))
        .await
        .unwrap();
    fiber_handle.ready().await.unwrap();
    assert_eq!(cleaned.load(Ordering::SeqCst), 0);

    fiber_handle.dispose().await.unwrap();
    assert_eq!(
        cleaned.load(Ordering::SeqCst),
        1,
        "the derived context's effect is owned by the plugin's Fiber generation"
    );
}

struct Layered;

impl Service for Layered {
    const NAME: &'static str = "layered";
}

impl ConfigurableService for Layered {
    type Config = String;
    type Layer = String;
    type Resolved = Vec<String>;
    type PrepareError = Infallible;
    type ComposeError = Infallible;

    fn prepare_config(config: Self::Config) -> Result<Self::Layer, Self::PrepareError> {
        Ok(config)
    }

    fn compose_config<'a>(
        base: Option<&'a Self::Layer>,
        layers: impl IntoIterator<Item = &'a Self::Layer>,
        head: Option<&'a Self::Layer>,
    ) -> Result<Self::Resolved, Self::ComposeError> {
        Ok(base
            .into_iter()
            .chain(layers)
            .chain(head)
            .cloned()
            .collect())
    }
}

struct RootOwned;

impl Service for RootOwned {
    const NAME: &'static str = "root-owned";
}

struct RootProbe {
    hits: Arc<AtomicUsize>,
}

impl Plugin for RootProbe {
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
        let root = ctx.root();
        assert!(
            root.resolve_config::<Layered>(None, None)
                .unwrap()
                .is_empty()
        );
        let _ = root.provide(Arc::new(RootOwned)).unwrap();
        let hits = self.hits.clone();
        let _listener = root
            .on::<Ping, _>(observer_sync(move |_, ()| -> Result<(), Infallible> {
                hits.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }))
            .unwrap();
        std::future::ready(Ok(()))
    }
}

#[tokio::test]
async fn root_resets_fiber_isolate_scope_and_intercept_in_the_same_runtime() {
    let runtime = Context::new();
    let origin = runtime
        .with_isolated_service(RootOwned::NAME)
        .with_intercept::<Layered>("origin".to_owned());
    assert_eq!(
        origin.resolve_config::<Layered>(None, None).unwrap(),
        ["origin"]
    );

    let hits = Arc::new(AtomicUsize::new(0));
    let fiber_handle = origin
        .spawn(PreparedPlugin::from_input(
            RootProbe { hits: hits.clone() },
            (),
        ))
        .await
        .unwrap();

    assert!(runtime.try_service::<RootOwned>().is_ok());
    assert!(origin.try_service::<RootOwned>().is_err());
    runtime
        .emit::<Ping>(Routing::Scoped(runtime.scope()), ())
        .await
        .unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 1, "root() reset Scope");

    fiber_handle.dispose().await.unwrap();
    assert!(runtime.try_service::<RootOwned>().is_ok());
    runtime
        .emit::<Ping>(Routing::Scoped(runtime.scope()), ())
        .await
        .unwrap();
    assert_eq!(
        hits.load(Ordering::SeqCst),
        2,
        "root() reset Fiber attribution to the permanent root Fiber"
    );
}

struct CloneOwned;

impl Service for CloneOwned {
    const NAME: &'static str = "clone-owned";
}

struct CloneProbe {
    scope: Arc<Mutex<Option<Scope>>>,
    hits: Arc<AtomicUsize>,
}

impl Plugin for CloneProbe {
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
        let cloned = ctx.clone();
        assert_eq!(
            cloned.resolve_config::<Layered>(None, None).unwrap(),
            ["origin"]
        );
        let _ = cloned.provide(Arc::new(CloneOwned)).unwrap();

        let hits = self.hits.clone();
        let _listener = cloned
            .on::<Ping, _>(observer_sync(move |_, ()| -> Result<(), Infallible> {
                hits.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }))
            .unwrap();
        *self.scope.lock() = Some(cloned.scope());
        std::future::ready(Ok(()))
    }
}

#[tokio::test]
async fn clone_preserves_fiber_realm_scope_and_intercept_exactly() {
    let runtime = Context::new();
    let origin = runtime
        .with_isolated_service(CloneOwned::NAME)
        .with_intercept::<Layered>("origin".to_owned());
    let scope = Arc::new(Mutex::new(None));
    let hits = Arc::new(AtomicUsize::new(0));
    let fiber_handle = origin
        .spawn(PreparedPlugin::from_input(
            CloneProbe {
                scope: scope.clone(),
                hits: hits.clone(),
            },
            (),
        ))
        .await
        .unwrap();

    assert!(origin.try_service::<CloneOwned>().is_ok());
    assert!(runtime.try_service::<CloneOwned>().is_err());
    let exact_scope = scope
        .lock()
        .clone()
        .expect("clone probe stores its exact Scope");
    runtime
        .emit::<Ping>(Routing::Scoped(exact_scope.clone()), ())
        .await
        .unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    runtime
        .emit::<Ping>(Routing::Scoped(runtime.scope()), ())
        .await
        .unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    fiber_handle.dispose().await.unwrap();
    assert!(origin.try_service::<CloneOwned>().is_err());
    runtime
        .emit::<Ping>(Routing::Scoped(exact_scope.clone()), ())
        .await
        .unwrap();
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "clone registrations retain the plugin Fiber's disposal ownership"
    );
}
