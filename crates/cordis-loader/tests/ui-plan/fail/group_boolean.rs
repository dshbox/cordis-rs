use cordis_loader::PluginEntry;
fn main() {
    let mut entry = PluginEntry::new(serde_json::Value::Null);
    entry.group = true;
}
