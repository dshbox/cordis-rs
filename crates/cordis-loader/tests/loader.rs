//! Loader end-to-end behavior over real temp files.

use cordis::{Context, Fiber, FiberState, Inject, PluginHandle, PluginOutput, plugin_sync};
use cordis_include::{Document, EntryOptions, Node, PluginResolver};
use cordis_loader::{Loader, LoaderConfig, PluginRegistry};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn temp_path(stem: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("cordis-loader-test-{stem}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("cordis.yml")
}

fn cleanup(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir_all(parent);
    }
}

/// A plugin that records starts and reads a `port` from its Node config.
fn counting_plugin(name: &'static str, starts: Arc<AtomicUsize>) -> PluginHandle {
    plugin_sync::<Node, _>(name, Inject::default(), move |_ctx, config| {
        starts.fetch_add(1, Ordering::SeqCst);
        let _port = config["port"].as_i64();
        Ok(PluginOutput::none())
    })
}

#[test]
fn open_starts_enabled_entries_and_skips_disabled_ones() {
    let path = temp_path("open");
    let starts = Arc::new(AtomicUsize::new(0));
    let mut registry = PluginRegistry::new();
    registry.register("worker", {
        let starts = starts.clone();
        move || counting_plugin("worker", starts.clone())
    });
    let initial = Document::with_entries(vec![
        EntryOptions::new("worker")
            .with_id("w1")
            .with_config(Node::from_iter([("port".to_string(), 8080.into())])),
        EntryOptions::new("worker")
            .with_id("w2")
            .with_disabled(true),
    ]);
    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&path)
            .with_registry(registry)
            .with_initial(initial),
    )
    .unwrap();

    let w1 = loader.tree().resolve("w1").unwrap();
    let fiber = w1.fiber().unwrap();
    fiber.try_wait().unwrap();
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    assert!(loader.tree().resolve("w2").unwrap().fiber().is_none());
    assert!(loader.last_error().is_none());

    // The initial document was persisted with the generated state intact.
    let reread = loader.file().read().unwrap();
    assert!(
        reread
            .entries
            .iter()
            .any(|options| options.id.as_deref() == Some("w1"))
    );
    loader.dispose().unwrap();
    cleanup(&path);
}

#[test]
fn reload_reconciles_created_removed_updated_and_moved() {
    let path = temp_path("reload");
    let starts = Arc::new(AtomicUsize::new(0));
    let mut registry = PluginRegistry::new();
    registry.register("worker", {
        let starts = starts.clone();
        move || counting_plugin("worker", starts.clone())
    });
    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&path)
            .with_registry(registry)
            .with_initial(Document::with_entries(vec![
                EntryOptions::new("worker").with_id("keep"),
                EntryOptions::new("worker").with_id("drop"),
            ])),
    )
    .unwrap();
    assert_eq!(starts.load(Ordering::SeqCst), 2);

    // External edit: "keep" gets a new port, "drop" disappears, "new"
    // appears inside a group that itself is new.
    std::fs::write(
        &path,
        "entries:\n  - id: keep\n    name: worker\n    config:\n      port: 9090\n  - id: grp\n    name: group\n    group:\n      - id: new\n        name: worker\n",
    )
    .unwrap();
    let diff = loader.reload().unwrap();
    assert!(diff.created.iter().any(|e| e.id() == "grp"));
    assert!(diff.created.iter().any(|e| e.id() == "new"));
    assert!(diff.updated.iter().any(|e| e.id() == "keep"));
    assert!(diff.removed.iter().any(|e| e.id() == "drop"));

    // Config-only change patched the fiber without a restart: starts went
    // 2 -> 3 (update_value restarts "keep"), and the new entries started.
    assert_eq!(starts.load(Ordering::SeqCst), 4);
    let keep = loader.tree().resolve("keep").unwrap();
    let config = keep.fiber().unwrap().config().downcast::<Node>().unwrap();
    assert_eq!(config["port"].as_i64(), Some(9090));
    assert!(loader.tree().resolve("drop").is_none());
    assert!(loader.tree().resolve("grp:new").unwrap().fiber().is_some());

    // Removing the group cascades to its child.
    std::fs::write(&path, "entries:\n  - id: keep\n    name: worker\n").unwrap();
    loader.reload().unwrap();
    assert!(loader.tree().resolve("grp").is_none());
    let child_gone = loader
        .tree()
        .entries()
        .iter()
        .all(|entry| entry.id() != "new" || entry.fiber().is_none());
    assert!(child_gone);
    loader.dispose().unwrap();
    cleanup(&path);
}

#[test]
fn self_disposed_plugin_is_disabled_in_the_file() {
    let path = temp_path("selfkill");
    let victim_fiber: Arc<Mutex<Option<Fiber>>> = Arc::new(Mutex::new(None));
    let mut registry = PluginRegistry::new();
    registry.register("victim", {
        let victim_fiber = victim_fiber.clone();
        move || {
            let victim_fiber = victim_fiber.clone();
            plugin_sync::<Node, _>("victim", Inject::default(), move |ctx, _config| {
                *victim_fiber.lock().unwrap() = Some(ctx.fiber()?);
                Ok(PluginOutput::none())
            })
        }
    });
    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&path)
            .with_registry(registry)
            .with_initial(Document::with_entries(vec![
                EntryOptions::new("victim").with_id("v1"),
            ])),
    )
    .unwrap();
    let entry = loader.tree().resolve("v1").unwrap();
    entry.fiber().unwrap().try_wait().unwrap();

    // The plugin tears itself down outside the loader.
    victim_fiber
        .lock()
        .unwrap()
        .clone()
        .unwrap()
        .dispose()
        .unwrap();

    assert!(entry.fiber().is_none());
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("disabled: true"), "{text}");
    // Loader-driven disposals must NOT be misclassified: stop the loader and
    // confirm no further rewrite happened beyond the persisted state.
    let before = std::fs::read_to_string(&path).unwrap();
    loader.dispose().unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    cleanup(&path);
}

#[test]
fn entry_level_inject_waits_for_the_service() {
    let path = temp_path("inject");
    let starts = Arc::new(AtomicUsize::new(0));
    let mut registry = PluginRegistry::new();
    registry.register("consumer", {
        let starts = starts.clone();
        move || {
            let starts = starts.clone();
            plugin_sync::<Node, _>("consumer", Inject::default(), move |ctx, _config| {
                starts.fetch_add(1, Ordering::SeqCst);
                let _service = ctx.require::<u32>("svc")?;
                Ok(PluginOutput::none())
            })
        }
    });
    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&path)
            .with_registry(registry)
            .with_initial(Document::with_entries(vec![
                EntryOptions::new("consumer")
                    .with_id("c1")
                    .with_inject(["svc"]),
            ])),
    )
    .unwrap();
    let entry = loader.tree().resolve("c1").unwrap();
    assert_eq!(entry.fiber().unwrap().state(), FiberState::Pending);

    // Service appears: the entry activates through the core machinery.
    let _svc = root.provide("svc", 7_u32).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if entry.fiber().unwrap().state() == FiberState::Active {
            break;
        }
        assert!(Instant::now() < deadline, "entry never became active");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(starts.load(Ordering::SeqCst), 1);

    // Service goes away: the entry demotes to Pending again.
    _svc.dispose().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if entry.fiber().unwrap().state() == FiberState::Pending {
            break;
        }
        assert!(Instant::now() < deadline, "entry never returned to pending");
        std::thread::sleep(Duration::from_millis(20));
    }
    loader.dispose().unwrap();
    cleanup(&path);
}

#[test]
fn update_config_restarts_the_fiber_and_persists() {
    let path = temp_path("update");
    let starts = Arc::new(AtomicUsize::new(0));
    let mut registry = PluginRegistry::new();
    registry.register("worker", {
        let starts = starts.clone();
        move || counting_plugin("worker", starts.clone())
    });
    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&path)
            .with_registry(registry)
            .with_initial(Document::with_entries(vec![
                EntryOptions::new("worker")
                    .with_id("w1")
                    .with_config(Node::from_iter([("port".to_string(), 1.into())])),
            ])),
    )
    .unwrap();
    loader
        .tree()
        .resolve("w1")
        .unwrap()
        .fiber()
        .unwrap()
        .try_wait()
        .unwrap();

    let new_config = Node::from_iter([("port".to_string(), 2.into())]);
    loader.update_config("w1", new_config).unwrap();

    let entry = loader.tree().resolve("w1").unwrap();
    entry.fiber().unwrap().try_wait().unwrap();
    let config = entry.fiber().unwrap().config().downcast::<Node>().unwrap();
    assert_eq!(config["port"].as_i64(), Some(2));
    assert_eq!(starts.load(Ordering::SeqCst), 2);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("port: 2"), "{text}");
    loader.dispose().unwrap();
    cleanup(&path);
}

#[test]
fn registry_resolves_distinct_identities_and_rejects_unknown_names() {
    let registry = PluginRegistry::new();
    let group = registry.resolve("group").unwrap();
    let group_again = registry.resolve("group").unwrap();
    assert_ne!(group.key(), group_again.key());
    assert!(registry.resolve("nope").is_err());
}

#[test]
fn loader_is_exposed_as_a_weak_service() {
    let path = temp_path("service");
    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&path).with_initial(Document::default()),
    )
    .unwrap();
    let handle = root
        .require::<cordis_loader::LoaderHandle>("loader")
        .unwrap();
    assert!(handle.upgrade().is_some());
    drop(loader);
    assert!(handle.upgrade().is_none());
    cleanup(&path);
}
