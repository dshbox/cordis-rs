//! Issue 40 / CX-08 black-box evidence: Context behavior is determined by
//! Runtime, current Fiber, and the semantic isolate / Scope / intercept positions,
//! never by how an equivalent view was allocated or derived.

use cordis_core::event::{ListenerOptions, mapper_sync, observer_sync};
use cordis_core::{
    ConfigurableService, Context, Event, Plugin, PreparedChange, PreparedPlugin, Routing, Service,
    ServiceRealm,
};
use parking_lot::Mutex;
use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct LookupProbe(&'static str);
impl Service for LookupProbe {
    const NAME: &'static str = "issue40/lookup";
}

struct ViewConfig;
impl Service for ViewConfig {
    const NAME: &'static str = "issue40/config";
}
impl ConfigurableService for ViewConfig {
    type Config = usize;
    type Layer = usize;
    type Resolved = usize;
    type PrepareError = Infallible;
    type ComposeError = Infallible;

    fn prepare_config(config: usize) -> Result<Self::Layer, Self::PrepareError> {
        Ok(config)
    }

    fn compose_config<'a>(
        base: Option<&'a Self::Layer>,
        layers: impl IntoIterator<Item = &'a Self::Layer>,
        head: Option<&'a Self::Layer>,
    ) -> Result<Self::Resolved, Self::ComposeError> {
        Ok(base.into_iter().chain(layers).chain(head).copied().sum())
    }
}

fn equivalent_views(base: &Context, realm: &ServiceRealm) -> (Context, Context) {
    let direct = base
        .with_service_realms([(LookupProbe::NAME, realm.clone())])
        .unwrap()
        .with_intercept::<ViewConfig>(7);
    let repeated = base
        .with_service_realms([(LookupProbe::NAME, realm.clone())])
        .unwrap()
        .with_service_realms([(LookupProbe::NAME, realm.clone())])
        .unwrap()
        .with_intercept::<ViewConfig>(7);
    (direct, repeated)
}

struct Ping;
impl Event for Ping {
    const NAME: &'static str = "issue40/ping";
    type Args = ();
    type Output = ();
}

#[tokio::test]
async fn equivalent_views_match_for_service_lookup_and_event_routing() {
    let root = Context::new();
    let realm = root.new_service_realm();
    let other_realm = root.new_service_realm();
    let publisher = root
        .with_service_realms([(LookupProbe::NAME, realm.clone())])
        .unwrap();
    let _publication = publisher.provide(Arc::new(LookupProbe("joined"))).unwrap();

    let semantic_scope = root.with_child_scope();
    let sibling_scope = root.with_child_scope();
    let (direct, repeated) = equivalent_views(&semantic_scope, &realm);

    assert_eq!(direct.try_service::<LookupProbe>().unwrap().0, "joined");
    assert_eq!(repeated.try_service::<LookupProbe>().unwrap().0, "joined");
    assert_eq!(direct.resolve_config::<ViewConfig>(None, None).unwrap(), 7);
    assert_eq!(
        repeated.resolve_config::<ViewConfig>(None, None).unwrap(),
        7
    );

    let different_isolate = semantic_scope
        .with_service_realms([(LookupProbe::NAME, other_realm)])
        .unwrap()
        .with_intercept::<ViewConfig>(7);
    assert!(different_isolate.try_service::<LookupProbe>().is_err());

    let different_intercept = semantic_scope
        .with_service_realms([(LookupProbe::NAME, realm.clone())])
        .unwrap()
        .with_intercept::<ViewConfig>(8);
    assert_eq!(
        different_intercept
            .resolve_config::<ViewConfig>(None, None)
            .unwrap(),
        8,
        "a genuinely different intercept position changes configuration behavior"
    );

    let seen = Arc::new(Mutex::new(Vec::new()));
    for (view, label) in [(&direct, "direct"), (&repeated, "repeated")] {
        let seen = seen.clone();
        view.on::<Ping, _>(observer_sync(move |registration_ctx: Context, ()| {
            let service = registration_ctx.try_service::<LookupProbe>().unwrap();
            let config = registration_ctx
                .resolve_config::<ViewConfig>(None, None)
                .unwrap();
            seen.lock().push((label, service.0, config));
            Ok::<(), Infallible>(())
        }))
        .unwrap();
    }

    root.emit::<Ping>(Routing::Scoped(repeated.scope()), ())
        .await
        .unwrap();
    assert_eq!(
        &*seen.lock(),
        &[("direct", "joined", 7), ("repeated", "joined", 7)]
    );

    root.emit::<Ping>(Routing::Scoped(sibling_scope.scope()), ())
        .await
        .unwrap();
    assert_eq!(
        seen.lock().len(),
        2,
        "a genuinely different Scope position, unlike derivation history, changes reachability"
    );
}

#[derive(Clone)]
struct UpdateProbe {
    applied: Arc<Mutex<Vec<u8>>>,
}
impl Plugin for UpdateProbe {
    type Config = u8;
    type Input = u8;
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, config: u8) -> Result<u8, Infallible> {
        Ok(config)
    }

    fn apply(
        &self,
        _ctx: Context,
        input: &u8,
    ) -> impl Future<Output = Result<(), Infallible>> + Send {
        self.applied.lock().push(*input);
        std::future::ready(Ok(()))
    }
}

#[tokio::test]
async fn equivalent_views_match_for_typed_update_control() {
    let root = Context::new();
    let realm = root.new_service_realm();
    let (direct, repeated) = equivalent_views(&root, &realm);
    let sibling = root.with_child_scope();
    let policy_contexts = Arc::new(Mutex::new(Vec::new()));

    for view in [&direct, &repeated] {
        let policy_contexts = policy_contexts.clone();
        view.on_update::<UpdateProbe, _>(
            mapper_sync::<UpdateProbe, _>(move |registration_ctx: Context, value: u8| {
                policy_contexts.lock().push(
                    registration_ctx
                        .resolve_config::<ViewConfig>(None, None)
                        .unwrap(),
                );
                Ok::<_, Infallible>(value + 1)
            }),
            ListenerOptions::default(),
        )
        .unwrap();
    }

    let sibling_hits = Arc::new(AtomicUsize::new(0));
    let sibling_hit = sibling_hits.clone();
    sibling
        .on_update::<UpdateProbe, _>(
            mapper_sync::<UpdateProbe, _>(move |_ctx: Context, value: u8| {
                sibling_hit.fetch_add(1, Ordering::SeqCst);
                Ok::<_, Infallible>(value + 100)
            }),
            ListenerOptions::default(),
        )
        .unwrap();

    let applied = Arc::new(Mutex::new(Vec::new()));
    let fiber_handle = root
        .spawn(PreparedPlugin::from_input(
            UpdateProbe {
                applied: applied.clone(),
            },
            1,
        ))
        .await
        .unwrap();

    fiber_handle
        .update(PreparedChange::from_input::<UpdateProbe>(10))
        .await
        .unwrap();
    assert_eq!(&*applied.lock(), &[1, 12]);
    assert_eq!(&*policy_contexts.lock(), &[7, 7]);
    assert_eq!(
        sibling_hits.load(Ordering::SeqCst),
        0,
        "a genuinely different Scope position does not control the target Fiber"
    );
}

struct CleanupProbe {
    realm: ServiceRealm,
    direct_cleaned: Arc<AtomicUsize>,
    repeated_cleaned: Arc<AtomicUsize>,
    root_cleaned: Arc<AtomicUsize>,
    root_registration: Arc<Mutex<Option<cordis_core::effect::EffectRegistration>>>,
}

impl Plugin for CleanupProbe {
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
        let (direct, repeated) = equivalent_views(&ctx, &self.realm);

        let direct_cleaned = self.direct_cleaned.clone();
        direct
            .effect_sync(move || {
                direct_cleaned.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();

        let repeated_cleaned = self.repeated_cleaned.clone();
        repeated
            .effect_sync(move || {
                repeated_cleaned.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();

        let root_cleaned = self.root_cleaned.clone();
        let registration = ctx
            .root()
            .effect_sync(move || {
                root_cleaned.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();
        *self.root_registration.lock() = Some(registration);

        std::future::ready(Ok(()))
    }
}

#[tokio::test]
async fn equivalent_views_register_cleanup_to_the_same_current_fiber() {
    let root = Context::new();
    let direct_cleaned = Arc::new(AtomicUsize::new(0));
    let repeated_cleaned = Arc::new(AtomicUsize::new(0));
    let root_cleaned = Arc::new(AtomicUsize::new(0));
    let root_registration = Arc::new(Mutex::new(None));
    let fiber_handle = root
        .spawn(PreparedPlugin::from_input(
            CleanupProbe {
                realm: root.new_service_realm(),
                direct_cleaned: direct_cleaned.clone(),
                repeated_cleaned: repeated_cleaned.clone(),
                root_cleaned: root_cleaned.clone(),
                root_registration: root_registration.clone(),
            },
            (),
        ))
        .await
        .unwrap();

    fiber_handle.dispose().await.unwrap();
    assert_eq!(direct_cleaned.load(Ordering::SeqCst), 1);
    assert_eq!(repeated_cleaned.load(Ordering::SeqCst), 1);
    assert_eq!(
        root_cleaned.load(Ordering::SeqCst),
        0,
        "ownership follows the explicitly selected current Fiber, not derivation ancestry"
    );

    let registration = root_registration.lock().take().unwrap();
    assert!(registration.dispose().await.unwrap());
    assert_eq!(root_cleaned.load(Ordering::SeqCst), 1);
}
