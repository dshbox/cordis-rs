//! Strict Loader source schema at the public loading boundary
//! (`serde_json` text → [`PluginEntry`] → [`LoadPlanBuilder`] → `LoadPlan::load`).
//!
//! A misspelled or misplaced source field must be rejected at deserialization.
//! Silently discarding it falls back to a default that changes what executes:
//! a `disable` typo runs the Plugin, and an `isolated` typo places a private
//! tenant Service into the caller's realm. Valid rows, including every
//! documented wire form, keep parsing and round-trip unchanged.

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cordis_core::service::{Service, ServicePublishError};
use cordis_core::{Context, FiberState, Plugin, PreparedPlugin};
use cordis_loader::outcome::{EntryOutcome, LoadOutcome};
use cordis_loader::plan::{
    EntryGroup, InjectEntry, IsolateEntry, LoadPlanBuilder, PluginEntry, RealmPolicy,
};
use cordis_loader::resolver::PluginRequest;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

struct TenantCache;
impl Service for TenantCache {
    const NAME: &'static str = "strict-schema/tenant-cache";
}

/// Counts applies and publishes one tenant Service into its placed realm.
struct Tenant(Arc<AtomicUsize>);

impl Plugin for Tenant {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = ServicePublishError;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _: &()) -> Result<(), ServicePublishError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        let _publication = ctx.provide::<TenantCache>(Arc::new(TenantCache))?;
        Ok(())
    }
}

async fn load_row(ctx: &Context, row: &str, applies: &Arc<AtomicUsize>) -> LoadOutcome {
    let entry: PluginEntry = serde_json::from_str(row).expect("valid source row parses");
    let mut builder = LoadPlanBuilder::new();
    builder.add_plugin(None, entry).unwrap();
    let plan = builder.finish().unwrap();
    let applies = applies.clone();
    let resolver = move |request: PluginRequest<'_>| -> Result<Option<PreparedPlugin>, Infallible> {
        assert_eq!(request.resolve_key(), "tenant");
        Ok(Some(PreparedPlugin::from_input(
            Tenant(applies.clone()),
            (),
        )))
    };
    plan.load(ctx, &resolver).await
}

fn assert_rejects<T>(row: &str, field: &str)
where
    T: DeserializeOwned + std::fmt::Debug,
{
    let error = serde_json::from_str::<T>(row).expect_err(&format!(
        "{} must reject unknown field `{field}` instead of discarding it: {row}",
        std::any::type_name::<T>()
    ));
    let message = error.to_string();
    assert!(
        message.contains(&format!("unknown field `{field}`")),
        "rejection must name the offending field `{field}`, got: {message}"
    );
}

#[test]
fn misspelled_disabled_is_rejected() {
    assert_rejects::<PluginEntry>(
        r#"{"name": "tenant", "config": null, "disable": true}"#,
        "disable",
    );
}

/// Positive control: the correctly spelled flag disables execution.
#[tokio::test]
async fn disabled_row_does_not_execute() {
    let ctx = Context::new();
    let applies = Arc::new(AtomicUsize::new(0));
    let outcome = load_row(
        &ctx,
        r#"{"name": "tenant", "config": null, "disabled": true}"#,
        &applies,
    )
    .await;
    assert!(matches!(outcome.entries(), [EntryOutcome::Disabled { .. }]));
    assert_eq!(applies.load(Ordering::SeqCst), 0);
}

#[test]
fn misspelled_isolate_is_rejected() {
    assert_rejects::<PluginEntry>(
        r#"{"name": "tenant", "config": null,
            "isolated": [{"service": "strict-schema/tenant-cache", "policy": {"kind": "private"}}]}"#,
        "isolated",
    );
}

/// Positive control: the correctly spelled row keeps the tenant Service out of
/// the caller's realm.
#[tokio::test]
async fn private_isolate_row_keeps_the_service_out_of_the_caller_realm() {
    let ctx = Context::new();
    let applies = Arc::new(AtomicUsize::new(0));
    let outcome = load_row(
        &ctx,
        r#"{"name": "tenant", "config": null,
            "isolate": [{"service": "strict-schema/tenant-cache", "policy": {"kind": "private"}}]}"#,
        &applies,
    )
    .await;
    let handles = outcome.fiber_handles().cloned().collect::<Vec<_>>();
    assert_eq!(handles.len(), 1);
    assert_eq!(handles[0].state(), FiberState::Active);
    assert_eq!(applies.load(Ordering::SeqCst), 1);
    assert!(
        ctx.try_service::<TenantCache>().is_err(),
        "a private tenant Service must not be visible from the caller's realm"
    );
    for handle in handles {
        handle.dispose().await.unwrap();
    }
}

/// Upstream-style fields that this Loader does not implement are reported
/// rather than silently ignored.
#[test]
fn unsupported_plugin_entry_fields_are_rejected() {
    for field in ["id", "group", "intercept"] {
        let mut row = json!({"name": "tenant", "config": null});
        row[field] = json!(true);
        assert_rejects::<PluginEntry>(&row.to_string(), field);
    }
}

#[test]
fn fields_belonging_to_another_row_or_variant_are_rejected() {
    // `Private` is a unit variant in Rust; its wire form must still reject a
    // `label` that only `shared` carries.
    assert_rejects::<RealmPolicy>(r#"{"kind": "private", "label": "tenant-a"}"#, "label");
    assert_rejects::<RealmPolicy>(
        r#"{"kind": "shared", "label": "tenant-a", "scope": "global"}"#,
        "scope",
    );
    assert_rejects::<InjectEntry>(
        r#"{"kind": "required", "service": "db", "config": {"pool": 8}}"#,
        "config",
    );
    assert_rejects::<InjectEntry>(
        r#"{"kind": "configured", "service": "db", "config": null, "optional": true}"#,
        "optional",
    );
    assert_rejects::<IsolateEntry>(
        r#"{"service": "db", "policy": {"kind": "private"}, "shared": true}"#,
        "shared",
    );
    assert_rejects::<EntryGroup>(r#"{"name": "workers", "disabled": true}"#, "disabled");
}

/// Strictness is exact to field names: the tag itself and every documented
/// field still parse, and serialization output is unchanged.
#[test]
fn documented_rows_parse_and_round_trip() {
    let full = json!({
        "key": "worker",
        "name": "primary",
        "config": {"threads": 2},
        "disabled": true,
        "inject": [
            {"kind": "required", "service": "logger"},
            {"kind": "configured", "service": "db", "config": {"pool": 8}}
        ],
        "isolate": [
            {"service": "cache", "policy": {"kind": "private"}},
            {"service": "db", "policy": {"kind": "shared", "label": "tenant-a"}}
        ]
    });
    let entry: PluginEntry = serde_json::from_value(full.clone()).unwrap();
    assert_eq!(serde_json::to_value(&entry).unwrap(), full);

    // Omitted optional sequences and flags keep their defaults.
    let minimal: PluginEntry =
        serde_json::from_str(r#"{"key": "worker", "config": null}"#).unwrap();
    assert!(!minimal.disabled && minimal.inject.is_empty() && minimal.isolate.is_empty());
    assert_eq!(
        serde_json::to_value(&minimal).unwrap(),
        json!({"key": "worker", "name": null, "config": null,
               "disabled": false, "inject": [], "isolate": []})
    );

    // Explicit-null key form used by the gateway example.
    let gateway: PluginEntry = serde_json::from_str(
        r#"{"name":"guard","key":null,"config":{"lock_auth":true},"disabled":false,"inject":[],"isolate":[]}"#,
    )
    .unwrap();
    assert_eq!(gateway.name.as_deref(), Some("guard"));

    let group: EntryGroup = serde_json::from_str(r#"{"name": "workers"}"#).unwrap();
    assert_eq!(
        serde_json::to_value(&group).unwrap(),
        json!({"name": "workers"})
    );

    for (policy, wire) in [
        (RealmPolicy::Private, json!({"kind": "private"})),
        (
            RealmPolicy::Shared { label: "x".into() },
            json!({"kind": "shared", "label": "x"}),
        ),
    ] {
        assert_eq!(serde_json::to_value(&policy).unwrap(), wire);
        let parsed: RealmPolicy = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), wire);
    }
    for (inject, wire) in [
        (
            InjectEntry::Required("a".into()),
            json!({"kind": "required", "service": "a"}),
        ),
        (
            InjectEntry::Configured {
                service: "b".into(),
                config: Value::Null,
            },
            json!({"kind": "configured", "service": "b", "config": null}),
        ),
    ] {
        assert_eq!(serde_json::to_value(&inject).unwrap(), wire);
        let parsed: InjectEntry = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), wire);
    }
}
