//! The loader/entry-* event family and debounced write-backs.

use cordis::{Context, Inject, plugin_sync};
use cordis_include::{Document, EntryOptions, Node};
use cordis_loader::events;
use cordis_loader::{Loader, LoaderConfig, PluginRegistry};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn temp_dir(stem: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("cordis-events-test-{stem}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn worker_registry() -> PluginRegistry {
    let mut registry = PluginRegistry::new();
    registry.register("worker", || {
        plugin_sync::<Node, _>("worker", Inject::default(), |_, _| {
            Ok(cordis::PluginOutput::none())
        })
    });
    registry
}

#[derive(Default)]
struct Counts {
    entry_init: AtomicUsize,
    before_patch: AtomicUsize,
    after_patch: AtomicUsize,
    config_update: AtomicUsize,
    partial_dispose: AtomicUsize,
    patched_ids: Mutex<Vec<String>>,
}

fn listen(root: &Context, counts: &Arc<Counts>) {
    let bus = root.events();
    let c = counts.clone();
    bus.on(
        events::ENTRY_INIT,
        move |event| {
            let entry = event.arg::<cordis_include::Entry>(0)?.expect("entry arg");
            c.entry_init.fetch_add(1, Ordering::SeqCst);
            assert!(!entry.id().is_empty());
            Ok(None)
        },
        cordis::EventOptions::default(),
    )
    .unwrap();
    let c = counts.clone();
    bus.on(
        events::BEFORE_PATCH,
        move |_| {
            c.before_patch.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        },
        cordis::EventOptions::default(),
    )
    .unwrap();
    let c = counts.clone();
    bus.on(
        events::AFTER_PATCH,
        move |event| {
            c.after_patch.fetch_add(1, Ordering::SeqCst);
            let entry = event.arg::<cordis_include::Entry>(0)?.expect("entry arg");
            lock(&c.patched_ids).push(entry.id().to_string());
            Ok(None)
        },
        cordis::EventOptions::default(),
    )
    .unwrap();
    let c = counts.clone();
    bus.on(
        events::CONFIG_UPDATE,
        move |event| {
            c.config_update.fetch_add(1, Ordering::SeqCst);
            let entry = event.arg::<cordis_include::Entry>(0)?.expect("entry arg");
            let config = event.arg::<Node>(1)?.expect("config arg");
            assert!(!entry.id().is_empty());
            assert_eq!(config["port"].as_i64(), Some(9));
            Ok(None)
        },
        cordis::EventOptions::default(),
    )
    .unwrap();
    let c = counts.clone();
    bus.on(
        events::PARTIAL_DISPOSE,
        move |_| {
            c.partial_dispose.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        },
        cordis::EventOptions::default(),
    )
    .unwrap();
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

#[test]
fn loader_events_trace_the_state_machine() {
    let dir = temp_dir("family");
    let config_path = dir.join("cordis.yml");
    let counts = Arc::new(Counts::default());

    let root = Context::new();
    listen(&root, &counts);
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&config_path)
            .with_registry(worker_registry())
            .with_initial(Document::with_entries(vec![
                EntryOptions::new("worker").with_id("w1"),
            ])),
    )
    .unwrap();
    assert_eq!(counts.entry_init.load(Ordering::SeqCst), 1);

    // Config-only reload edit -> before/after patch, no new init.
    std::fs::write(
        &config_path,
        "entries:\n  - id: w1\n    name: worker\n    config:\n      port: 5\n",
    )
    .unwrap();
    loader.reload().unwrap();
    assert_eq!(counts.entry_init.load(Ordering::SeqCst), 1);
    assert_eq!(counts.before_patch.load(Ordering::SeqCst), 1);
    assert_eq!(counts.after_patch.load(Ordering::SeqCst), 1);
    assert_eq!(*lock(&counts.patched_ids), vec!["w1".to_string()]);

    // Runtime config change -> config-update.
    loader
        .update_config(
            "w1",
            [("port".to_string(), Node::Int(9))].into_iter().collect(),
        )
        .unwrap();
    assert_eq!(counts.config_update.load(Ordering::SeqCst), 1);

    // Removing the entry stops it without a partial-dispose (that is for
    // self-killed plugins only).
    std::fs::write(&config_path, "entries:\n").unwrap();
    loader.reload().unwrap();
    assert_eq!(counts.partial_dispose.load(Ordering::SeqCst), 0);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn self_kill_emits_partial_dispose() {
    let dir = temp_dir("selfkill");
    let config_path = dir.join("cordis.yml");
    let victim: Arc<Mutex<Option<cordis::Fiber>>> = Arc::new(Mutex::new(None));
    let mut registry = PluginRegistry::new();
    registry.register("victim", {
        let victim = victim.clone();
        move || {
            let victim = victim.clone();
            plugin_sync::<Node, _>("victim", Inject::default(), move |ctx, _| {
                *lock(&victim) = Some(ctx.fiber()?);
                Ok(cordis::PluginOutput::none())
            })
        }
    });

    let root = Context::new();
    // Bound with `_` so the loader (and its status listener) stays alive
    // for the whole test even though this test never touches it directly.
    let _loader = Loader::open(
        &root,
        LoaderConfig::new(&config_path)
            .with_registry(registry)
            .with_initial(Document::with_entries(vec![
                EntryOptions::new("victim").with_id("v1"),
            ])),
    )
    .unwrap();
    let counts = Arc::new(Counts::default());
    // Register after open so the earlier lifecycle does not count.
    listen(&root, &counts);

    lock(&victim).clone().unwrap().dispose().unwrap();
    // The self-kill persistence (and its PARTIAL_DISPOSE emission) is
    // deferred off the dying fiber's transition lock; poll for it.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let persisted = std::fs::read_to_string(&config_path)
            .unwrap()
            .contains("disabled: true");
        if counts.partial_dispose.load(Ordering::SeqCst) == 1 && persisted {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "self-kill events/persistence never landed"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn debounced_write_back_coalesces_updates() {
    let dir = temp_dir("debounce");
    let config_path = dir.join("cordis.yml");

    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&config_path)
            .with_registry(worker_registry())
            .with_write_debounce(Duration::from_millis(300))
            .with_initial(Document::with_entries(vec![
                EntryOptions::new("worker").with_id("w1"),
            ])),
    )
    .unwrap();

    for port in [1, 2, 3] {
        loader
            .update_config(
                "w1",
                [("port".to_string(), Node::Int(port))]
                    .into_iter()
                    .collect(),
            )
            .unwrap();
    }
    loader.file().flush_deferred();

    let text = std::fs::read_to_string(&config_path).unwrap();
    assert!(text.contains("port: 3"), "latest config persisted: {text}");
    assert!(
        !text.contains("port: 1"),
        "stale writes coalesced away: {text}"
    );
    assert!(loader.file().last_deferred_error().is_none());

    let _ = std::fs::remove_dir_all(&dir);
}
