//! Issue 49 behavioral evidence for immutable validated Loader plans.

use std::collections::HashSet;

use cordis_loader::plan::{
    EntryGroup, InjectEntry, IsolateEntry, LoadPlanBuilder, PlanError, PluginEntry, RealmPolicy,
};
use serde_json::{Value, json};

fn plugin(key: &str) -> PluginEntry {
    let mut entry = PluginEntry::new(Value::Null);
    entry.key = Some(key.to_owned());
    entry
}

#[test]
fn builder_accepts_only_already_admitted_same_lineage_parents_and_freezes_ids() {
    let mut builder = LoadPlanBuilder::new();
    let root = builder.add_group(None, EntryGroup::new("root")).unwrap();
    let plugin_id = builder.add_plugin(Some(&root), plugin("worker")).unwrap();
    let nested_group = builder
        .add_group(Some(&plugin_id), EntryGroup::new("nested"))
        .unwrap();

    let mut foreign = LoadPlanBuilder::new();
    let foreign_parent = foreign.add_group(None, EntryGroup::new("foreign")).unwrap();
    let err = builder
        .add_plugin(Some(&foreign_parent), plugin("orphan"))
        .unwrap_err();
    assert!(matches!(err, PlanError::ForeignParent { parent } if parent == foreign_parent));

    let plan = builder.finish().unwrap();
    let clone = plan.clone();
    assert_eq!(root, root.clone());
    assert_eq!(plugin_id, plugin_id.clone());
    drop((plan, clone));

    let mut rebuilt = LoadPlanBuilder::new();
    let rebuilt_root = rebuilt.add_group(None, EntryGroup::new("root")).unwrap();
    let rebuilt_plugin = rebuilt
        .add_plugin(Some(&rebuilt_root), plugin("worker"))
        .unwrap();
    let rebuilt_nested = rebuilt
        .add_group(Some(&rebuilt_plugin), EntryGroup::new("nested"))
        .unwrap();
    rebuilt.finish().unwrap();

    assert_ne!(
        root, rebuilt_root,
        "equal source reconstructed independently has a new lineage"
    );
    assert_ne!(plugin_id, rebuilt_plugin);
    assert_ne!(nested_group, rebuilt_nested);
}

#[test]
fn failed_add_is_atomic_and_its_id_is_not_an_admitted_parent() {
    let mut builder = LoadPlanBuilder::new();
    let root = builder.add_group(None, EntryGroup::new("root")).unwrap();

    let mut invalid = plugin("bad");
    invalid.inject = vec![
        InjectEntry::Required("db".into()),
        InjectEntry::Configured {
            service: "db".into(),
            config: json!({"pool": 4}),
        },
    ];
    let rejected = match builder.add_plugin(Some(&root), invalid).unwrap_err() {
        PlanError::DuplicateInjectService { entry, service } => {
            assert_eq!(service, "db");
            entry
        }
        other => panic!("unexpected error: {other:?}"),
    };

    let err = builder
        .add_group(Some(&rejected), EntryGroup::new("cannot-attach"))
        .unwrap_err();
    assert!(matches!(err, PlanError::ForeignParent { parent } if parent == rejected));

    let good = builder.add_plugin(Some(&root), plugin("good")).unwrap();
    assert_ne!(
        rejected, good,
        "a rejected attempt never aliases a later admitted entry"
    );
    assert_ne!(root, good);
    builder.finish().unwrap();
}

#[test]
fn validation_rejects_missing_identity_and_duplicate_axes_but_keeps_axes_orthogonal() {
    let mut builder = LoadPlanBuilder::new();

    let missing = PluginEntry::new(Value::Null);
    assert!(matches!(
        builder.add_plugin(None, missing).unwrap_err(),
        PlanError::MissingResolveIdentity { .. }
    ));

    let mut duplicate_inject = plugin("inject");
    duplicate_inject.inject = vec![
        InjectEntry::Required("cache".into()),
        InjectEntry::Required("cache".into()),
    ];
    assert!(matches!(
        builder.add_plugin(None, duplicate_inject).unwrap_err(),
        PlanError::DuplicateInjectService { service, .. } if service == "cache"
    ));

    let mut duplicate_isolate = plugin("isolate");
    duplicate_isolate.isolate = vec![
        IsolateEntry {
            service: "db".into(),
            policy: RealmPolicy::Private,
        },
        IsolateEntry {
            service: "db".into(),
            policy: RealmPolicy::Shared {
                label: "tenant".into(),
            },
        },
    ];
    assert!(matches!(
        builder.add_plugin(None, duplicate_isolate).unwrap_err(),
        PlanError::DuplicateIsolateService { service, .. } if service == "db"
    ));

    let mut orthogonal = plugin("orthogonal");
    orthogonal.inject = vec![InjectEntry::Required("db".into())];
    orthogonal.isolate = vec![IsolateEntry {
        service: "db".into(),
        policy: RealmPolicy::Private,
    }];
    builder.add_plugin(None, orthogonal).unwrap();
    builder.finish().unwrap();
}

#[test]
fn source_schema_has_explicit_stable_wire_forms_and_required_plugin_config() {
    let plugin_json = json!({
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
    let entry: PluginEntry = serde_json::from_value(plugin_json.clone()).unwrap();
    assert_eq!(serde_json::to_value(&entry).unwrap(), plugin_json);

    let group_json = json!({"name": "workers"});
    let group: EntryGroup = serde_json::from_value(group_json.clone()).unwrap();
    assert_eq!(serde_json::to_value(&group).unwrap(), group_json);

    let required: InjectEntry = serde_json::from_value(json!({
        "kind": "required", "service": "cache"
    }))
    .unwrap();
    assert!(matches!(required, InjectEntry::Required(service) if service == "cache"));

    let configured: InjectEntry = serde_json::from_value(json!({
        "kind": "configured", "service": "db", "config": null
    }))
    .unwrap();
    assert!(
        matches!(configured, InjectEntry::Configured { service, config } if service == "db" && config.is_null())
    );

    let private: RealmPolicy = serde_json::from_value(json!({"kind": "private"})).unwrap();
    assert!(matches!(private, RealmPolicy::Private));
    let shared: RealmPolicy = serde_json::from_value(json!({
        "kind": "shared", "label": "tenant-a"
    }))
    .unwrap();
    assert!(matches!(shared, RealmPolicy::Shared { label } if label == "tenant-a"));

    let missing_config = json!({
        "key": "worker",
        "disabled": false,
        "inject": [],
        "isolate": []
    });
    let err = serde_json::from_value::<PluginEntry>(missing_config).unwrap_err();
    assert!(
        err.to_string().contains("config"),
        "config must be present even for unit/null"
    );
}

#[test]
fn entry_id_supports_only_correlation_semantics_at_runtime() {
    let mut builder = LoadPlanBuilder::new();
    let a = builder.add_plugin(None, plugin("a")).unwrap();
    let b = builder.add_plugin(None, plugin("b")).unwrap();
    let a_clone = a.clone();

    assert_eq!(a, a_clone);
    assert_ne!(a, b);
    let ids: HashSet<_> = [a.clone(), a_clone, b.clone()].into_iter().collect();
    assert_eq!(ids.len(), 2);

    assert!(!format!("{a:?}").is_empty());
    builder.finish().unwrap();
}
