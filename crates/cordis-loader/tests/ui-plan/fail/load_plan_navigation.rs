use cordis_loader::{EntryGroup, LoadPlanBuilder};
fn main() {
    let mut builder = LoadPlanBuilder::new();
    let id = builder.add_group(None, EntryGroup::new("root")).unwrap();
    let plan = builder.finish().unwrap();
    let _ = plan.entries();
    let _ = plan.children(&id);
    let _ = plan.is_enabled(&id);
}
