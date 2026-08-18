//! Regression tests for the review findings on loader races and
//! self-kill persistence (issues #30, #33, #39).

use cordis::utils::BoxFuture;
use cordis::{
    Config, Context, CordisError, ErrorCode, Inject, Plugin, PluginHandle, PluginOutput, Result,
    plugin_sync,
};
use cordis_include::{Document, EntryOptions, Node};
use cordis_loader::{Loader, LoaderConfig, PluginRegistry};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn temp_path(stem: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cordis-loader-review-{stem}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("cordis.yml")
}

fn cleanup(path: &std::path::Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir_all(parent);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

/// A plugin whose validate_config rejects `port: 999`, counting how many
/// times validation ran so retries are observable.
struct PickyPlugin {
    validations: Arc<AtomicUsize>,
}

impl Plugin for PickyPlugin {
    fn name(&self) -> &str {
        "picky"
    }

    fn validate_config(&self, config: Config) -> Result<Config> {
        self.validations.fetch_add(1, Ordering::SeqCst);
        let node = config
            .downcast::<Node>()
            .map_err(|_| CordisError::new(ErrorCode::Plugin))?;
        if node["port"].as_i64() == Some(999) {
            return Err(CordisError::with_message(
                ErrorCode::Plugin,
                "port 999 is reserved",
            ));
        }
        Ok(Config::new((*node).clone()))
    }

    fn apply(&self, _ctx: Context, config: Config) -> BoxFuture<Result<PluginOutput>> {
        let node = config.downcast::<Node>().ok();
        Box::pin(async move {
            let _ = node;
            Ok(PluginOutput::none())
        })
    }
}

/// Regression (#30): a config the plugin rejects must not silently split
/// fiber / tree / file. The fiber keeps running its previous config, and
/// the next reload of the same file retries the patch instead of diffing
/// against already-committed options and doing nothing.
#[test]
fn rejected_patch_keeps_fiber_config_and_is_retried() {
    let path = temp_path("patch-retry");
    let validations = Arc::new(AtomicUsize::new(0));
    let mut registry = PluginRegistry::new();
    registry.register("picky", {
        let validations = validations.clone();
        move || {
            PluginHandle::new(PickyPlugin {
                validations: validations.clone(),
            })
        }
    });
    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&path)
            .with_registry(registry)
            .with_initial(Document::with_entries(vec![
                EntryOptions::new("picky")
                    .with_id("p1")
                    .with_config(Node::from_iter([("port".to_string(), 1.into())])),
            ])),
    )
    .unwrap();
    let entry = loader.tree().resolve("p1").unwrap();
    entry.fiber().unwrap().try_wait().unwrap();
    let validations_after_start = validations.load(Ordering::SeqCst);

    // External edit to a config the plugin rejects.
    std::fs::write(
        &path,
        "entries:\n  - id: p1\n    name: picky\n    config:\n      port: 999\n",
    )
    .unwrap();
    loader.reload().unwrap();
    let port = entry.fiber().unwrap().config().downcast::<Node>().unwrap();
    assert_eq!(port["port"].as_i64(), Some(1), "fiber keeps its old config");
    assert!(loader.last_error().is_some(), "the rejection is recorded");
    assert_eq!(
        validations.load(Ordering::SeqCst),
        validations_after_start + 1,
        "the patch attempt validated the new config"
    );

    // Reloading the SAME file must retry the rejected patch. Before the
    // fix, the tree already held the rejected options, the diff was empty,
    // and the fiber was pinned to the stale config forever.
    loader.reload().unwrap();
    assert_eq!(
        validations.load(Ordering::SeqCst),
        validations_after_start + 2,
        "a same-file reload retries the patch"
    );
    let port = entry.fiber().unwrap().config().downcast::<Node>().unwrap();
    assert_eq!(port["port"].as_i64(), Some(1));

    // A subsequent valid config patches the fiber successfully.
    std::fs::write(
        &path,
        "entries:\n  - id: p1\n    name: picky\n    config:\n      port: 2\n",
    )
    .unwrap();
    loader.reload().unwrap();
    let port = entry.fiber().unwrap().config().downcast::<Node>().unwrap();
    assert_eq!(port["port"].as_i64(), Some(2));
    loader.dispose().unwrap();
    cleanup(&path);
}

/// Regression (#33): a plugin-thread update_config() racing watch-thread
/// reloads must never leave the fiber serving a config that differs from
/// the tree and the file.
#[test]
fn update_config_racing_reload_stays_consistent() {
    let path = temp_path("race");
    let starts = Arc::new(AtomicUsize::new(0));
    let mut registry = PluginRegistry::new();
    registry.register("worker", {
        let starts = starts.clone();
        move || {
            let starts = starts.clone();
            plugin_sync::<Node, _>("worker", Inject::default(), move |_ctx, config| {
                starts.fetch_add(1, Ordering::SeqCst);
                let _port = config["port"].as_i64();
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
                EntryOptions::new("worker")
                    .with_id("w1")
                    .with_config(Node::from_iter([("port".to_string(), 1.into())])),
            ])),
    )
    .unwrap();
    let entry = loader.tree().resolve("w1").unwrap();
    entry.fiber().unwrap().try_wait().unwrap();

    let racer = loader.clone();
    let updates = std::thread::spawn(move || {
        for port in 2..=51_i64 {
            racer
                .update_config("w1", Node::from_iter([("port".to_string(), port.into())]))
                .expect("update_config must not fail under concurrent reloads");
        }
    });
    for _ in 0..50 {
        loader.reload().unwrap();
    }
    updates.join().unwrap();

    // Tree, fiber, and file must agree on the final config.
    let tree_port = loader.tree().resolve("w1").unwrap().config().unwrap()["port"].as_i64();
    let fiber_port = entry.fiber().unwrap().config().downcast::<Node>().unwrap()["port"].as_i64();
    let file_port = loader.file().read().unwrap().entries[0]
        .config
        .clone()
        .unwrap()["port"]
        .as_i64();
    assert_eq!(tree_port, Some(51), "tree options");
    assert_eq!(fiber_port, Some(51), "fiber config");
    assert_eq!(file_port, Some(51), "file content");
    loader.dispose().unwrap();
    cleanup(&path);
}

/// Regression (#39): self-kill persistence (tree write + file fsync +
/// PARTIAL_DISPOSE listeners) used to run inside the dying fiber's
/// transition critical section. A slow listener must not stall the
/// disposing call — and the persistence must still land.
#[test]
fn self_kill_persistence_leaves_the_transition_snappy() {
    let path = temp_path("snappy");
    let victim: Arc<Mutex<Option<cordis::Fiber>>> = Arc::new(Mutex::new(None));
    let mut registry = PluginRegistry::new();
    registry.register("victim", {
        let victim = victim.clone();
        move || {
            let victim = victim.clone();
            plugin_sync::<Node, _>("victim", Inject::default(), move |ctx, _| {
                *lock(&victim) = Some(ctx.fiber()?);
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
    lock(&victim).clone().unwrap().try_wait().unwrap();

    let _slow = root
        .events()
        .on(
            cordis_loader::events::PARTIAL_DISPOSE,
            |_| {
                std::thread::sleep(Duration::from_secs(2));
                Ok(None)
            },
            cordis::EventOptions::default(),
        )
        .unwrap();

    let started = Instant::now();
    lock(&victim).clone().unwrap().dispose().unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(1500),
        "dispose stalled {elapsed:?} behind deferred persistence work"
    );

    // The persistence still lands, just off the transition lock.
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        if std::fs::read_to_string(&path)
            .unwrap()
            .contains("disabled: true")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "self-kill persistence never landed"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    loader.dispose().unwrap();
    cleanup(&path);
}
