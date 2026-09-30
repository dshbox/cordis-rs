use cordis_loader::{EntryGroup, EntryId, LoadPlan, LoadPlanBuilder, PluginEntry};
use cordis_loader::plan::{InjectEntry, IsolateEntry, PlanError, RealmPolicy};

fn correlate(_: &EntryId, _: &EntryId) {}
fn accept_plan(_: LoadPlan) {}
fn accept_error(_: PlanError) {}

fn main() {
    let mut builder = LoadPlanBuilder::new();
    let group = builder.add_group(None, EntryGroup::new("root")).unwrap();
    let mut entry = PluginEntry::new(serde_json::Value::Null);
    entry.key = Some("worker".into());
    entry.inject = vec![InjectEntry::Required("db".into())];
    entry.isolate = vec![IsolateEntry { service: "db".into(), policy: RealmPolicy::Private }];
    let plugin = builder.add_plugin(Some(&group), entry).unwrap();
    correlate(&group, &plugin);
    accept_plan(builder.finish().unwrap());
    let _ = accept_error;
}
