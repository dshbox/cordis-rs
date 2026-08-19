//! Regression tests for review findings: loader races and self-kill
//! persistence (issues #30, #33, #39) and the 2026-08-19 review
//! (REV-20260819-01/-02/-05).

use cordis::utils::BoxFuture;
use cordis::{
    Config, Context, CordisError, ErrorCode, FiberState, Inject, Plugin, PluginHandle,
    PluginOutput, Result, plugin_sync,
};
use cordis_include::{Document, Entry, EntryOptions, Node};
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

/// A registry with one no-op `worker` plugin, shared by the tests below.
fn worker_registry() -> PluginRegistry {
    let mut registry = PluginRegistry::new();
    registry.register("worker", || {
        plugin_sync::<Node, _>("worker", Inject::default(), |_, _| Ok(PluginOutput::none()))
    });
    registry
}

/// Regression (review 2026-08-19, REV-01): a reload moving a top-level
/// entry into a group used to clear the entry's parent link, so its fiber
/// restarted under the ROOT context — disposing the group no longer
/// cascaded to the moved child, and group isolation/intercept were lost.
#[test]
fn reload_moving_entry_into_group_keeps_cascade_dispose() {
    let path = temp_path("move-cascade");
    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&path)
            .with_registry(worker_registry())
            .with_initial(Document::with_entries(vec![
                EntryOptions::new("worker").with_id("w1"),
            ])),
    )
    .unwrap();
    let worker = loader.tree().resolve("w1").unwrap();
    worker.fiber().unwrap().try_wait().unwrap();

    // External edit: the top-level worker moves into a new group.
    std::fs::write(
        &path,
        "entries:\n  - id: g\n    name: group\n    group:\n      - id: w1\n        name: worker\n",
    )
    .unwrap();
    let diff = loader.reload().unwrap();
    assert!(diff.moved.iter().any(|entry| entry.id() == "w1"));

    let group = loader.tree().resolve("g").unwrap();
    let moved = loader.tree().resolve("g:w1").unwrap();
    assert_eq!(moved.path(), "g:w1");
    assert!(
        moved
            .parent()
            .is_some_and(|parent| Entry::ptr_eq(&parent, &group)),
        "the moved entry stays linked to its group"
    );
    moved.fiber().unwrap().try_wait().unwrap();

    // The moved fiber runs under the group's context: disposing the group
    // cascades. Before the fix it restarted under the root and stayed
    // Active here.
    let group_fiber = group.fiber().unwrap();
    let moved_fiber = moved.fiber().unwrap();
    group_fiber.dispose().unwrap();
    assert_eq!(
        moved_fiber.state(),
        FiberState::Disposed,
        "group disposal must cascade to the moved child"
    );

    loader.dispose().unwrap();
    cleanup(&path);
}

/// Regression (review 2026-08-19, REV-02): the reload dirty check only
/// looked at top-level rows, so ids generated for id-less entries nested
/// inside groups were never written back — every reload destroyed and
/// recreated those fibers under fresh random ids.
#[test]
fn reload_persists_generated_ids_of_nested_entries() {
    let path = temp_path("nested-ids");
    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&path)
            .with_registry(worker_registry())
            .with_initial(Document::with_entries(vec![
                EntryOptions::new("group")
                    .with_id("g")
                    .with_group(vec![EntryOptions::new("worker")]),
            ])),
    )
    .unwrap();
    let worker = loader.tree().top_level()[0].children()[0].clone();
    worker.fiber().unwrap().try_wait().unwrap();

    // The first reload materializes the generated id into the file (the
    // open-time id churns once — a no-id row can never match a pooled
    // entry — and the write-back persists the fresh one).
    loader.reload().unwrap();
    let worker = loader.tree().top_level()[0].children()[0].clone();
    let persisted = loader.file().read().unwrap().entries[0].group[0].id.clone();
    assert_eq!(
        persisted.as_deref(),
        Some(worker.id()),
        "the nested entry's generated id must land in the file"
    );
    worker.fiber().unwrap().try_wait().unwrap();
    let fiber_before = worker.fiber().unwrap();

    // The second reload matches by that id: no churn, same fiber.
    let diff = loader.reload().unwrap();
    assert!(
        diff.created.is_empty() && diff.removed.is_empty(),
        "second reload must not churn the nested entry: {diff:?}"
    );
    assert!(
        worker
            .fiber()
            .is_some_and(|fiber| cordis::Fiber::ptr_eq(&fiber, &fiber_before)),
        "the nested entry keeps its fiber across reloads"
    );

    loader.dispose().unwrap();
    cleanup(&path);
}

/// Regression (review 2026-08-19, REV-05): `Loader::open` kept only the
/// last compose error; with several broken imports the rest vanished
/// silently behind fix-and-retry loops. (A merely *missing* import file
/// composes as empty by design — the rows here exist but do not parse.)
#[test]
fn open_records_every_broken_import() {
    let path = temp_path("import-errors");
    let dir = path.parent().unwrap().to_path_buf();
    std::fs::write(
        dir.join("broken-a.yml"),
        "entries:\n  - id: w1\n    name: [\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("broken-b.yml"),
        "entries:\n  - id: w2\n    name: [\n",
    )
    .unwrap();
    let url = |stem: &str| {
        Node::from_iter([(
            "url".to_string(),
            dir.join(stem).to_string_lossy().into_owned().into(),
        )])
    };
    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&path)
            // The import builtin resolves the marker rows.
            .with_registry(PluginRegistry::new())
            .with_initial(Document::with_entries(vec![
                EntryOptions::new("import")
                    .with_id("ia")
                    .with_config(url("broken-a.yml")),
                EntryOptions::new("import")
                    .with_id("ib")
                    .with_config(url("broken-b.yml")),
            ])),
    )
    .unwrap();
    let error = loader.last_error().expect("import failures recorded");
    assert!(error.contains("broken-a.yml"), "{error}");
    assert!(error.contains("broken-b.yml"), "{error}");

    loader.dispose().unwrap();
    cleanup(&path);
}
