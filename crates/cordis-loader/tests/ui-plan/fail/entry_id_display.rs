use cordis_loader::{EntryGroup, LoadPlanBuilder};
fn main() {
    let mut builder = LoadPlanBuilder::new();
    let id = builder.add_group(None, EntryGroup::new("root")).unwrap();
    let _ = format!("{id}");
}
