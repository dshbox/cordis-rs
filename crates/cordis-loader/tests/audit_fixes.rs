//! Regression tests for audited loader defects: pure moves across groups
//! (P1), diamond imports (P2), releasing the loader on dispose (P3), inject
//! merge and structural redefines (P6), and corrupt main files.

use cordis::{Context, FiberState, Inject, PluginOutput, plugin_sync};
use cordis_loader::{Loader, LoaderConfig, PluginRegistry};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn temp_dir(stem: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cordis-audit-fix-{stem}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &std::path::Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

/// A counting plugin named `name`, applying to `starts`.
fn counting_registry(name: &'static str, starts: Arc<AtomicUsize>) -> PluginRegistry {
    let mut registry = PluginRegistry::new();
    registry.register(name, move || {
        let starts = starts.clone();
        plugin_sync::<cordis_loader::Node, _>(name, Inject::default(), move |_, _| {
            starts.fetch_add(1, Ordering::SeqCst);
            Ok(PluginOutput::none())
        })
    });
    registry
}

/// P1: an entry moved between groups without any options change must be
/// restarted under its new parent (it used to be stopped and forgotten).
#[test]
fn moving_an_entry_restarts_it_under_the_new_parent() {
    let dir = temp_dir("move");
    let config = dir.join("cordis.json");
    write(
        &config,
        r#"{"entries":[{"id":"grp","name":"group"},{"id":"w","name":"probe"}]}"#,
    );
    let starts = Arc::new(AtomicUsize::new(0));
    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&config).with_registry(counting_registry("probe", starts.clone())),
    )
    .unwrap();
    assert_eq!(starts.load(Ordering::SeqCst), 1);

    write(
        &config,
        r#"{"entries":[{"id":"grp","name":"group","group":[{"id":"w","name":"probe"}]}]}"#,
    );
    loader.reload().unwrap();

    assert_eq!(
        starts.load(Ordering::SeqCst),
        2,
        "the moved entry restarts under its new parent"
    );
    assert!(
        loader.tree().resolve("grp:w").unwrap().fiber().is_some(),
        "the moved entry runs again"
    );

    // A group move takes its descendants along.
    write(
        &config,
        r#"{"entries":[{"id":"w","name":"probe"},{"id":"grp","name":"group","group":[{"id":"c","name":"probe"}]}]}"#,
    );
    loader.reload().unwrap();
    write(
        &config,
        r#"{"entries":[{"id":"grp2","name":"group","group":[{"id":"c","name":"probe"}]}]}"#,
    );
    loader.reload().unwrap();
    assert!(
        loader.tree().resolve("grp2:c").unwrap().fiber().is_some(),
        "a child inside a moved group keeps running"
    );

    let _ = loader.dispose();
    let _ = std::fs::remove_dir_all(&dir);
}

/// P2: importing the same file from two parents (a diamond) is reported as
/// a duplicate mount — not as a cycle — and only real cycles keep the
/// "import cycle" diagnosis.
#[test]
fn diamond_imports_report_as_duplicate_mounts_not_cycles() {
    let dir = temp_dir("diamond");
    let main = dir.join("main.yml");
    write(
        &main,
        "entries:\n  - id: a\n    name: import\n    config:\n      url: a.yml\n  - id: b\n    name: import\n    config:\n      url: b.yml\n",
    );
    write(
        &dir.join("a.yml"),
        "entries:\n  - id: a-c\n    name: import\n    config:\n      url: c.yml\n",
    );
    write(
        &dir.join("b.yml"),
        "entries:\n  - id: b-c\n    name: import\n    config:\n      url: c.yml\n",
    );
    write(
        &dir.join("c.yml"),
        "entries:\n  - id: cw\n    name: group\n",
    );

    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&main).with_registry(PluginRegistry::new()),
    )
    .unwrap();

    // The first branch mounts; the diamond branch is dropped (entry ids
    // are globally unique, so the import graph must be a tree), but the
    // diagnosis names the actual problem instead of claiming a cycle.
    assert!(loader.tree().resolve("a:a-c").is_some());
    assert!(loader.tree().resolve("b:b-c").is_none());
    let error = loader.last_error().unwrap();
    assert!(error.contains("duplicate import"), "{error}");
    assert!(!error.contains("import cycle"), "{error}");

    let _ = loader.dispose();
    let _ = std::fs::remove_dir_all(&dir);
}

/// P3: disposing a loader releases its root-level effects, so a second
/// `Loader::open` on the same root works.
#[test]
fn dispose_releases_the_loader_for_a_second_open() {
    let dir = temp_dir("reopen");
    let config = dir.join("cordis.json");
    write(&config, r#"{"entries":[]}"#);

    let root = Context::new();
    let first = Loader::open(
        &root,
        LoaderConfig::new(&config).with_registry(PluginRegistry::new()),
    )
    .unwrap();
    first.dispose().unwrap();

    let second = Loader::open(
        &root,
        LoaderConfig::new(&config).with_registry(PluginRegistry::new()),
    );
    assert!(
        second.is_ok(),
        "a second open on the same root must work: {:?}",
        second.err().map(|error| error.to_string())
    );

    if let Ok(loader) = second {
        let _ = loader.dispose();
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// P6: an entry's inject declaration merges with the plugin's own — the
/// plugin still gates on its own dependencies.
#[test]
fn entry_inject_merges_with_the_plugins_own_dependencies() {
    let dir = temp_dir("merge");
    let config = dir.join("cordis.json");
    write(
        &config,
        r#"{"entries":[{"id":"m","name":"merged","inject":["http"]}]}"#,
    );

    let root = Context::new();
    let _http = root.provide("http", 80u16).unwrap();
    // `database` arrives only later.

    let started = Arc::new(AtomicUsize::new(0));
    let mut registry = PluginRegistry::new();
    let flag = started.clone();
    registry.register("merged", move || {
        let flag = flag.clone();
        plugin_sync::<cordis_loader::Node, _>("merged", Inject::new(["database"]), move |_, _| {
            flag.fetch_add(1, Ordering::SeqCst);
            Ok(PluginOutput::none())
        })
    });

    let loader = Loader::open(&root, LoaderConfig::new(&config).with_registry(registry)).unwrap();
    assert_eq!(
        started.load(Ordering::SeqCst),
        0,
        "the plugin waits for its own `database` dependency too"
    );
    let fiber = loader.tree().resolve("m").unwrap().fiber().unwrap();
    let merged: Vec<String> = fiber.inject().names().map(ToString::to_string).collect();
    assert!(merged.contains(&"http".to_owned()), "{merged:?}");
    assert!(merged.contains(&"database".to_owned()), "{merged:?}");

    let _database = root.provide("database", 1u8).unwrap();
    assert_eq!(
        started.load(Ordering::SeqCst),
        1,
        "dependency arrival activates the merged entry"
    );

    let _ = loader.dispose();
    let _ = std::fs::remove_dir_all(&dir);
}

/// P6: config-only changes patch in place (same fiber), structural changes
/// (entry inject) restart under a fresh fiber.
#[test]
fn config_changes_patch_in_place_and_inject_changes_restart() {
    let dir = temp_dir("redefined");
    let config = dir.join("cordis.json");
    write(
        &config,
        r#"{"entries":[{"id":"w","name":"probe","config":{"n":1}}]}"#,
    );

    let starts = Arc::new(AtomicUsize::new(0));
    let root = Context::new();
    let _svc = root.provide("svc", 1u8).unwrap();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&config).with_registry(counting_registry("probe", starts.clone())),
    )
    .unwrap();
    let uid = loader.tree().resolve("w").unwrap().fiber().unwrap().uid();

    // Config-only change: patched in place, the fiber keeps its identity.
    write(
        &config,
        r#"{"entries":[{"id":"w","name":"probe","config":{"n":2}}]}"#,
    );
    loader.reload().unwrap();
    assert_eq!(
        loader.tree().resolve("w").unwrap().fiber().unwrap().uid(),
        uid,
        "config-only changes keep the fiber"
    );

    // Entry inject change: structural, the entry restarts with a new fiber.
    write(
        &config,
        r#"{"entries":[{"id":"w","name":"probe","inject":["svc"],"config":{"n":2}}]}"#,
    );
    loader.reload().unwrap();
    assert_ne!(
        loader.tree().resolve("w").unwrap().fiber().unwrap().uid(),
        uid,
        "structural changes restart the fiber"
    );

    let _ = loader.dispose();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A failed reload must leave the last valid tree and its fibers untouched.
#[test]
fn a_corrupt_main_file_during_reload_preserves_running_tree() {
    let dir = temp_dir("corrupt-reload");
    let config = dir.join("cordis.yml");
    write(&config, "entries:\n  - id: w\n    name: probe\n");

    let starts = Arc::new(AtomicUsize::new(0));
    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&config).with_registry(counting_registry("probe", starts.clone())),
    )
    .unwrap();
    let entry = loader.tree().resolve("w").unwrap();
    let fiber = entry.fiber().unwrap();
    fiber.try_wait().unwrap();
    assert_eq!(fiber.state(), FiberState::Active);
    let uid = fiber.uid();
    let entry_count = loader.tree().entries().len();

    // A half-written main file is not a new empty configuration. The
    // reload reports the parse failure while preserving the live state.
    write(&config, "entries:\n  - id: w\n    name: [\n");
    let error = loader
        .reload()
        .expect_err("a corrupt main file must fail the reload");
    assert!(error.to_string().contains("parse"), "{error}");
    assert!(
        loader
            .last_error()
            .is_some_and(|error| error.contains("parse")),
        "reload error was not recorded: {:?}",
        loader.last_error()
    );
    assert_eq!(loader.tree().entries().len(), entry_count);
    let preserved = loader.tree().resolve("w").unwrap().fiber().unwrap();
    assert_eq!(preserved.state(), FiberState::Active);
    assert_eq!(preserved.uid(), uid);

    // Once the file is valid again, the unchanged entry is reused rather
    // than unnecessarily stopped and rebuilt.
    write(&config, "entries:\n  - id: w\n    name: probe\n");
    assert!(loader.reload().unwrap().is_empty());
    let recovered = loader.tree().resolve("w").unwrap().fiber().unwrap();
    assert_eq!(recovered.state(), FiberState::Active);
    assert_eq!(recovered.uid(), uid);
    assert_eq!(starts.load(Ordering::SeqCst), 1);

    let _ = loader.dispose();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A corrupt or unreadable main file fails `Loader::open` instead of
/// booting an empty tree (import files keep the tolerant path).
#[test]
fn a_corrupt_main_file_fails_open() {
    let dir = temp_dir("corrupt");
    let config = dir.join("cordis.json");
    write(&config, r#"{"entries":["#);

    let root = Context::new();
    let error = Loader::open(
        &root,
        LoaderConfig::new(&config).with_registry(PluginRegistry::new()),
    )
    .err()
    .expect("a corrupt main file must fail open");
    assert!(
        error.to_string().to_lowercase().contains("parse"),
        "{error}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
