//! Issue 52 contract evidence for per-execution Loader realm policy and reusable plans.

use std::convert::Infallible;
use std::marker::PhantomData;
use std::sync::Arc;

use cordis_core::service::{Service, ServicePublishError};
use cordis_core::{Context, Plugin, PreparedPlugin};
use cordis_loader::outcome::{EntryOutcome, LoaderFailure};
use cordis_loader::plan::{EntryGroup, IsolateEntry, LoadPlanBuilder, PluginEntry, RealmPolicy};
use cordis_loader::resolver::PluginRequest;

macro_rules! service {
    ($name:ident, $wire:literal) => {
        #[derive(Default)]
        struct $name;
        impl Service for $name {
            const NAME: &'static str = $wire;
        }
    };
}

service!(SharedLeft, "issue52/shared-left");
service!(SharedRight, "issue52/shared-right");
service!(DifferentLabel, "issue52/different-label");
service!(PrivateLeft, "issue52/private-left");
service!(PrivateRight, "issue52/private-right");
service!(GroupChild, "issue52/group-child");
service!(PluginChild, "issue52/plugin-child");
service!(MappedExact, "issue52/mapped-exact");
service!(UnmappedCompanion, "issue52/unmapped-companion");
service!(RootPeer, "issue52/root-peer");
service!(ReuseProbe, "issue52/reuse");
service!(ReusePrivate, "issue52/reuse-private");
service!(CollisionProbe, "issue52/collision");

struct Publish<S>(PhantomData<fn() -> S>);

impl<S> Default for Publish<S> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<S> Plugin for Publish<S>
where
    S: Service + Default,
{
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = ServicePublishError;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _: &()) -> Result<(), ServicePublishError> {
        let _publication = ctx.provide::<S>(Arc::new(S::default()))?;
        Ok(())
    }
}

struct PublishPair;
impl Plugin for PublishPair {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = ServicePublishError;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _: &()) -> Result<(), ServicePublishError> {
        let _mapped = ctx.provide::<MappedExact>(Arc::new(MappedExact))?;
        let _unmapped = ctx.provide::<UnmappedCompanion>(Arc::new(UnmappedCompanion))?;
        Ok(())
    }
}

fn plugin<S: Service>(key: &str, policy: Option<RealmPolicy>) -> PluginEntry {
    PluginEntry {
        key: Some(key.to_owned()),
        name: None,
        config: serde_json::Value::Null,
        disabled: false,
        inject: Vec::new(),
        isolate: policy
            .into_iter()
            .map(|policy| IsolateEntry {
                service: S::NAME.to_owned(),
                policy,
            })
            .collect(),
    }
}

fn prepared<S>() -> PreparedPlugin
where
    S: Service + Default,
{
    PreparedPlugin::from_input(Publish::<S>::default(), ())
}

fn resolver(request: PluginRequest<'_>) -> Result<Option<PreparedPlugin>, Infallible> {
    let prepared = match request.resolve_key() {
        "shared-left" => prepared::<SharedLeft>(),
        "shared-right" => prepared::<SharedRight>(),
        "different" => prepared::<DifferentLabel>(),
        "private-left" => prepared::<PrivateLeft>(),
        "private-right" => prepared::<PrivateRight>(),
        "group-child" => prepared::<GroupChild>(),
        "plugin-child" => prepared::<PluginChild>(),
        "pair" => PreparedPlugin::from_input(PublishPair, ()),
        "root-peer" => prepared::<RootPeer>(),
        "reuse" => prepared::<ReuseProbe>(),
        "reuse-private" => prepared::<ReusePrivate>(),
        "collision-a" | "collision-b" | "collision-c" => prepared::<CollisionProbe>(),
        _ => return Ok(None),
    };
    Ok(Some(prepared))
}

fn fiber_handle_for<'a>(
    outcome: &'a cordis_loader::LoadOutcome,
    id: &cordis_loader::EntryId,
) -> &'a cordis_core::FiberHandle {
    match outcome.entry(id) {
        Some(EntryOutcome::Spawned { fiber_handle, .. }) => fiber_handle,
        _ => panic!("expected spawned row"),
    }
}

fn service_for_provider(
    ctx: &Context,
    provider: &cordis_core::FiberId,
) -> cordis_core::observation::ServiceSnapshot {
    ctx.runtime_snapshot()
        .services()
        .iter()
        .find(|service| service.provider() == provider)
        .cloned()
        .expect("spawned publisher must have one live service occurrence")
}

fn named_service_for_provider(
    ctx: &Context,
    provider: &cordis_core::FiberId,
    service_name: &str,
) -> cordis_core::observation::ServiceSnapshot {
    ctx.runtime_snapshot()
        .services()
        .iter()
        .find(|service| service.provider() == provider && service.service() == service_name)
        .cloned()
        .expect("spawned publisher must have the named live service occurrence")
}

async fn dispose_all(outcome: &cordis_loader::LoadOutcome) {
    for fiber_handle in outcome.fiber_handles() {
        fiber_handle.dispose().await.unwrap();
    }
}

#[tokio::test]
async fn realm_policy_is_execution_local_service_exact_and_independent_of_structure() {
    let mut builder = LoadPlanBuilder::new();
    let group = builder
        .add_group(
            None,
            EntryGroup {
                name: "group".into(),
            },
        )
        .unwrap();
    let shared_left = builder
        .add_plugin(
            Some(&group),
            plugin::<SharedLeft>(
                "shared-left",
                Some(RealmPolicy::Shared {
                    label: "team".into(),
                }),
            ),
        )
        .unwrap();
    let shared_right = builder
        .add_plugin(
            None,
            plugin::<SharedRight>(
                "shared-right",
                Some(RealmPolicy::Shared {
                    label: "team".into(),
                }),
            ),
        )
        .unwrap();
    let different = builder
        .add_plugin(
            None,
            plugin::<DifferentLabel>(
                "different",
                Some(RealmPolicy::Shared {
                    label: "other".into(),
                }),
            ),
        )
        .unwrap();
    let private_left = builder
        .add_plugin(
            None,
            plugin::<PrivateLeft>("private-left", Some(RealmPolicy::Private)),
        )
        .unwrap();
    let private_right = builder
        .add_plugin(
            None,
            plugin::<PrivateRight>("private-right", Some(RealmPolicy::Private)),
        )
        .unwrap();
    let group_child = builder
        .add_plugin(Some(&group), plugin::<GroupChild>("group-child", None))
        .unwrap();
    let plugin_child = builder
        .add_plugin(
            Some(&shared_left),
            plugin::<PluginChild>("plugin-child", None),
        )
        .unwrap();
    let root_peer = builder
        .add_plugin(None, plugin::<RootPeer>("root-peer", None))
        .unwrap();
    let pair = builder
        .add_plugin(
            None,
            plugin::<MappedExact>(
                "pair",
                Some(RealmPolicy::Shared {
                    label: "team".into(),
                }),
            ),
        )
        .unwrap();
    let plan = builder.finish().unwrap();

    let ctx = Context::new();
    let outcome = plan.load(&ctx, &resolver).await;
    assert!(outcome.is_ok());

    let realm = |id: &cordis_loader::EntryId| {
        let provider = fiber_handle_for(&outcome, id).id();
        service_for_provider(&ctx, &provider).realm().clone()
    };

    assert_eq!(
        realm(&shared_left),
        realm(&shared_right),
        "equal Shared labels rendezvous inside one execution"
    );
    assert_ne!(
        realm(&shared_left),
        realm(&different),
        "different Shared labels select different opaque realms"
    );
    assert_ne!(
        realm(&private_left),
        realm(&private_right),
        "each Private declaration receives a fresh realm"
    );
    assert_eq!(
        realm(&group_child),
        realm(&root_peer),
        "a structural group adds no realm"
    );
    assert_eq!(
        realm(&plugin_child),
        realm(&root_peer),
        "Plugin parentage does not imply realm inheritance"
    );

    let pair_provider = fiber_handle_for(&outcome, &pair).id();
    let mapped = named_service_for_provider(&ctx, &pair_provider, MappedExact::NAME);
    let unmapped = named_service_for_provider(&ctx, &pair_provider, UnmappedCompanion::NAME);
    assert_eq!(
        mapped.realm(),
        &realm(&shared_left),
        "the declared Service uses the execution-local Shared label realm"
    );
    assert_eq!(
        unmapped.realm(),
        &realm(&root_peer),
        "the Shared label does not become fallback for undeclared Services"
    );
    assert_ne!(mapped.realm(), unmapped.realm());

    dispose_all(&outcome).await;
}

#[tokio::test]
async fn shared_placement_collision_fails_only_that_row_and_later_rows_continue() {
    let mut builder = LoadPlanBuilder::new();
    let first = builder
        .add_plugin(
            None,
            plugin::<CollisionProbe>(
                "collision-a",
                Some(RealmPolicy::Shared {
                    label: "same".into(),
                }),
            ),
        )
        .unwrap();
    let colliding = builder
        .add_plugin(
            None,
            plugin::<CollisionProbe>(
                "collision-b",
                Some(RealmPolicy::Shared {
                    label: "same".into(),
                }),
            ),
        )
        .unwrap();
    let later = builder
        .add_plugin(
            None,
            plugin::<CollisionProbe>(
                "collision-c",
                Some(RealmPolicy::Shared {
                    label: "different".into(),
                }),
            ),
        )
        .unwrap();
    let plan = builder.finish().unwrap();

    let ctx = Context::new();
    let outcome = plan.load(&ctx, &resolver).await;

    assert!(matches!(
        outcome.entry(&first),
        Some(EntryOutcome::Spawned { .. })
    ));
    assert!(matches!(
        outcome.entry(&colliding),
        Some(EntryOutcome::Failed {
            failure: LoaderFailure::Spawn(cordis_core::lifecycle::SpawnError::InitialApply(_)),
            ..
        })
    ));
    assert!(matches!(
        outcome.entry(&later),
        Some(EntryOutcome::Spawned { .. })
    ));
    assert_eq!(outcome.fiber_handles().count(), 2);
    assert!(!outcome.is_ok());

    dispose_all(&outcome).await;
}

#[tokio::test]
async fn cloned_plan_reuse_preserves_entry_correlation_but_refreshes_runtime_identity() {
    let mut builder = LoadPlanBuilder::new();
    let group = builder
        .add_group(
            None,
            EntryGroup {
                name: "root".into(),
            },
        )
        .unwrap();
    let entry = builder
        .add_plugin(
            Some(&group),
            plugin::<ReuseProbe>(
                "reuse",
                Some(RealmPolicy::Shared {
                    label: "stable-text-not-stable-realm".into(),
                }),
            ),
        )
        .unwrap();
    let private_entry = builder
        .add_plugin(
            Some(&group),
            plugin::<ReusePrivate>("reuse-private", Some(RealmPolicy::Private)),
        )
        .unwrap();
    let plan = builder.finish().unwrap();
    let clone = plan.clone();

    let same_runtime = Context::new();
    let first = plan.load(&same_runtime, &resolver).await;
    let second = clone.load(&same_runtime, &resolver).await;

    for outcome in [&first, &second] {
        assert_eq!(outcome.entries().len(), 3);
        assert!(matches!(outcome.entries()[0], EntryOutcome::Group { ref id } if id == &group));
        assert!(
            matches!(outcome.entries()[1], EntryOutcome::Spawned { ref id, .. } if id == &entry)
        );
        assert!(
            matches!(outcome.entries()[2], EntryOutcome::Spawned { ref id, .. } if id == &private_entry)
        );
        assert!(outcome.entry(&entry).is_some());
        assert!(outcome.entry(&private_entry).is_some());
    }

    let first_fiber_handle = fiber_handle_for(&first, &entry);
    let second_fiber_handle = fiber_handle_for(&second, &entry);
    assert_ne!(
        first_fiber_handle.id(),
        second_fiber_handle.id(),
        "FiberId is execution-specific"
    );
    let first_service = service_for_provider(&same_runtime, &first_fiber_handle.id());
    let second_service = service_for_provider(&same_runtime, &second_fiber_handle.id());
    assert_ne!(
        first_service.realm(),
        second_service.realm(),
        "the same Shared text does not rendezvous across executions"
    );
    assert_ne!(
        first_service.id(),
        second_service.id(),
        "publication occurrences are fresh per execution"
    );
    let first_private_fiber_handle = fiber_handle_for(&first, &private_entry);
    let second_private_fiber_handle = fiber_handle_for(&second, &private_entry);
    let first_private = service_for_provider(&same_runtime, &first_private_fiber_handle.id());
    let second_private = service_for_provider(&same_runtime, &second_private_fiber_handle.id());
    assert_ne!(
        first_private.realm(),
        second_private.realm(),
        "Private policy allocates a fresh realm again for each execution"
    );

    first_fiber_handle.dispose().await.unwrap();
    first_private_fiber_handle.dispose().await.unwrap();
    let remaining = same_runtime.runtime_snapshot();
    assert!(
        remaining
            .services()
            .iter()
            .any(|service| service.provider() == &second_fiber_handle.id())
    );
    assert!(
        remaining
            .services()
            .iter()
            .all(|service| service.provider() != &first_fiber_handle.id())
    );

    let other_runtime = Context::new();
    let third = clone.load(&other_runtime, &resolver).await;
    assert!(matches!(third.entries()[0], EntryOutcome::Group { ref id } if id == &group));
    assert!(matches!(third.entries()[1], EntryOutcome::Spawned { ref id, .. } if id == &entry));
    assert!(
        matches!(third.entries()[2], EntryOutcome::Spawned { ref id, .. } if id == &private_entry)
    );
    let third_fiber_handle = fiber_handle_for(&third, &entry);
    let third_service = service_for_provider(&other_runtime, &third_fiber_handle.id());
    assert_ne!(second_fiber_handle.id(), third_fiber_handle.id());
    assert_ne!(second_service.realm(), third_service.realm());
    assert_ne!(second_service.id(), third_service.id());

    second_fiber_handle.dispose().await.unwrap();
    fiber_handle_for(&second, &private_entry)
        .dispose()
        .await
        .unwrap();
    third_fiber_handle.dispose().await.unwrap();
    fiber_handle_for(&third, &private_entry)
        .dispose()
        .await
        .unwrap();
}
