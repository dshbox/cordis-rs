use cordis_loader::{EntryGroup, LoadPlanBuilder};
fn main() {
    let builder = LoadPlanBuilder::new();
    let mut plan = builder.finish().unwrap();
    let _ = plan.add_group(None, EntryGroup::new("late"));
}
