//! Import entries: sub-files mounted as subtrees, reloaded and persisted
//! through the compose/decompose routing.

use cordis::{Context, Inject, plugin_sync};
use cordis_include::Node;
use cordis_loader::{Loader, LoaderConfig, PluginRegistry};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn temp_dir(stem: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("cordis-import-test-{stem}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn worker_registry(starts: Arc<AtomicUsize>) -> PluginRegistry {
    let mut registry = PluginRegistry::new();
    registry.register("worker", move || {
        let starts = starts.clone();
        plugin_sync::<Node, _>("worker", Inject::default(), move |_ctx, config| {
            starts.fetch_add(1, Ordering::SeqCst);
            let _port = config["port"].as_i64();
            Ok(cordis::PluginOutput::none())
        })
    });
    registry
}

#[test]
fn import_mounts_sub_file_entries_as_a_subtree() {
    let dir = temp_dir("mount");
    let main = dir.join("main.yml");
    let sub = dir.join("extra.yml");
    std::fs::write(
        &main,
        "entries:\n  - id: imp\n    name: import\n    config:\n      url: extra.yml\n",
    )
    .unwrap();
    std::fs::write(&sub, "entries:\n  - id: w1\n    name: worker\n").unwrap();

    let starts = Arc::new(AtomicUsize::new(0));
    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&main).with_registry(worker_registry(starts.clone())),
    )
    .unwrap();

    let import = loader.tree().resolve("imp").unwrap();
    import.fiber().unwrap().try_wait().unwrap(); // import marker is active
    let child = loader.tree().resolve("imp:w1").unwrap();
    child.fiber().unwrap().try_wait().unwrap();
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    assert_eq!(import.name(), "import");

    // Children run under the import fiber's context: disposing the import
    // cascades (here through a loader-driven group stop).
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn corrupt_import_is_recorded_and_skipped_without_failing_the_main_file() {
    let dir = temp_dir("corrupt");
    let main = dir.join("main.yml");
    let sub = dir.join("extra.yml");
    std::fs::write(
        &main,
        "entries:\n  - id: imp\n    name: import\n    config:\n      url: extra.yml\n",
    )
    .unwrap();
    std::fs::write(&sub, "entries:\n  - id: w1\n    name: [\n").unwrap();

    let starts = Arc::new(AtomicUsize::new(0));
    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&main).with_registry(worker_registry(starts.clone())),
    )
    .unwrap();

    assert!(loader.tree().resolve("imp").is_some());
    assert!(loader.tree().resolve("imp:w1").is_none());
    assert!(
        loader
            .last_error()
            .is_some_and(|error| error.contains("parse")),
        "import error was not recorded: {:?}",
        loader.last_error()
    );

    // Once the import is readable, a normal reload mounts and starts it.
    std::fs::write(&sub, "entries:\n  - id: w1\n    name: worker\n").unwrap();
    let diff = loader.reload().unwrap();
    assert!(diff.created.iter().any(|entry| entry.path() == "imp:w1"));
    assert!(loader.tree().resolve("imp:w1").unwrap().fiber().is_some());
    assert_eq!(starts.load(Ordering::SeqCst), 1);

    let _ = loader.dispose();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn import_sub_file_reload_diffs_only_its_children() {
    let dir = temp_dir("reload");
    let main = dir.join("main.yml");
    let sub = dir.join("extra.yml");
    std::fs::write(
        &main,
        "entries:\n  - id: imp\n    name: import\n    config:\n      url: extra.yml\n",
    )
    .unwrap();
    std::fs::write(
        &sub,
        "entries:\n  - id: keep\n    name: worker\n  - id: gone\n    name: worker\n",
    )
    .unwrap();

    let starts = Arc::new(AtomicUsize::new(0));
    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&main).with_registry(worker_registry(starts.clone())),
    )
    .unwrap();
    assert_eq!(starts.load(Ordering::SeqCst), 2);

    // External edit of the SUB file: remove one entry, patch the other.
    std::fs::write(
        &sub,
        "entries:\n  - id: keep\n    name: worker\n    config:\n      port: 7\n",
    )
    .unwrap();
    let diff = loader.reload().unwrap();
    assert!(diff.updated.iter().any(|e| e.path() == "imp:keep"));
    assert!(diff.removed.iter().any(|e| e.path == "imp:gone"));
    assert!(diff.created.is_empty());

    let keep = loader.tree().resolve("imp:keep").unwrap();
    let config = keep.fiber().unwrap().config().downcast::<Node>().unwrap();
    assert_eq!(config["port"].as_i64(), Some(7));
    assert!(loader.tree().resolve("imp:gone").is_none());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn generated_ids_persist_to_the_sub_file_not_the_main_file() {
    let dir = temp_dir("ids");
    let main = dir.join("main.yml");
    let sub = dir.join("extra.yml");
    std::fs::write(
        &main,
        "entries:\n  - id: imp\n    name: import\n    config:\n      url: extra.yml\n",
    )
    .unwrap();
    std::fs::write(&sub, "entries:\n  - name: worker\n").unwrap(); // no id

    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&main).with_registry(worker_registry(Arc::new(AtomicUsize::new(0)))),
    )
    .unwrap();
    std::fs::write(&sub, "entries:\n  - name: worker\n").unwrap(); // touch the sub file
    loader.reload().unwrap();

    let sub_text = std::fs::read_to_string(&sub).unwrap();
    let main_text = std::fs::read_to_string(&main).unwrap();
    assert!(
        sub_text.contains("id:"),
        "generated id persisted to sub file: {sub_text}"
    );
    assert!(
        !sub_text.contains("import"),
        "sub file stays flat: {sub_text}"
    );
    assert!(
        main_text.contains("url: extra.yml"),
        "main file untouched structurally: {main_text}"
    );
    assert!(
        !main_text.contains("name: worker"),
        "mounted children never leak into main: {main_text}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn import_cycles_are_reported_without_recursing_forever() {
    let dir = temp_dir("cycle");
    let a = dir.join("a.yml");
    let b = dir.join("b.yml");
    std::fs::write(
        &a,
        "entries:\n  - id: ia\n    name: import\n    config:\n      url: b.yml\n",
    )
    .unwrap();
    std::fs::write(
        &b,
        "entries:\n  - id: ib\n    name: import\n    config:\n      url: a.yml\n",
    )
    .unwrap();

    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&a).with_registry(worker_registry(Arc::new(AtomicUsize::new(0)))),
    )
    .unwrap();
    assert!(
        loader.last_error().unwrap().contains("cycle"),
        "cycle recorded: {:?}",
        loader.last_error()
    );
    // Both import entries still exist as (childless) markers.
    assert!(loader.tree().resolve("ia").is_some());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn update_config_on_a_mounted_entry_writes_the_sub_file() {
    let dir = temp_dir("update");
    let main = dir.join("main.yml");
    let sub = dir.join("extra.yml");
    std::fs::write(
        &main,
        "entries:\n  - id: imp\n    name: import\n    config:\n      url: extra.yml\n",
    )
    .unwrap();
    std::fs::write(&sub, "entries:\n  - id: w1\n    name: worker\n").unwrap();

    let starts = Arc::new(AtomicUsize::new(0));
    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&main).with_registry(worker_registry(starts.clone())),
    )
    .unwrap();
    loader
        .tree()
        .resolve("imp:w1")
        .unwrap()
        .fiber()
        .unwrap()
        .try_wait()
        .unwrap();

    loader
        .update_config(
            "imp:w1",
            [("port".to_string(), Node::Int(42))].into_iter().collect(),
        )
        .unwrap();

    let sub_text = std::fs::read_to_string(&sub).unwrap();
    let main_text = std::fs::read_to_string(&main).unwrap();
    assert!(
        sub_text.contains("port: 42"),
        "config persisted to the owning sub file: {sub_text}"
    );
    assert!(
        !main_text.contains("port: 42"),
        "main file does not absorb mounted entries: {main_text}"
    );
    let entry = loader.tree().resolve("imp:w1").unwrap();
    entry.fiber().unwrap().try_wait().unwrap();
    assert_eq!(starts.load(Ordering::SeqCst), 2); // patched, not re-created

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn disabled_import_cascades_to_mounted_children() {
    let dir = temp_dir("disable");
    let main = dir.join("main.yml");
    let sub = dir.join("extra.yml");
    std::fs::write(&main, "entries:\n  - id: imp\n    name: import\n    disabled: true\n    config:\n      url: extra.yml\n").unwrap();
    std::fs::write(&sub, "entries:\n  - id: w1\n    name: worker\n").unwrap();

    let starts = Arc::new(AtomicUsize::new(0));
    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&main).with_registry(worker_registry(starts.clone())),
    )
    .unwrap();
    let import = loader.tree().resolve("imp").unwrap();
    let child = loader.tree().resolve("imp:w1").unwrap();
    assert!(import.fiber().is_none(), "disabled import never starts");
    assert!(
        child.fiber().is_none(),
        "cascade keeps children from starting"
    );
    assert_eq!(starts.load(Ordering::SeqCst), 0);

    let _ = std::fs::remove_dir_all(&dir);
}
