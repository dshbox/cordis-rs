//! Document-backed composition sources: `LoaderConfig::with_document` and
//! `Loader::recompose` — boot from an in-memory document whose entry file is
//! only ever a write-back draft, and the in-memory recomposition primitive.

use cordis::{Context, Fiber, Inject, PluginHandle, PluginOutput, plugin_sync};
use cordis_include::{Document, EntryOptions, Node};
use cordis_loader::{Loader, LoaderConfig, PluginRegistry};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn temp_path(stem: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cordis-loader-doc-test-{stem}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("cordis.yml")
}

fn cleanup(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir_all(parent);
    }
}

/// A plugin that records starts.
fn counting_plugin(name: &'static str, starts: Arc<AtomicUsize>) -> PluginHandle {
    plugin_sync::<Node, _>(name, Inject::default(), move |_ctx, _config| {
        starts.fetch_add(1, Ordering::SeqCst);
        Ok(PluginOutput::none())
    })
}

fn counting_registry(starts: &Arc<AtomicUsize>) -> PluginRegistry {
    let mut registry = PluginRegistry::new();
    let starts = starts.clone();
    registry.register("worker", move || counting_plugin("worker", starts.clone()));
    registry
}

#[test]
fn open_composes_from_the_document_and_never_touches_the_file() {
    // The draft directory does not even exist: document-backed boot neither
    // reads nor writes anything, so read-only (or missing) directories work.
    let dir = std::env::temp_dir().join(format!(
        "cordis-loader-doc-test-missing-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("nested").join("cordis.yml");

    let starts = Arc::new(AtomicUsize::new(0));
    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&path)
            .with_registry(counting_registry(&starts))
            .with_document(Document::with_entries(vec![
                EntryOptions::new("worker").with_id("w1"),
            ])),
    )
    .unwrap();

    let entry = loader.tree().resolve("w1").unwrap();
    entry.fiber().unwrap().try_wait().unwrap();
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    assert!(!dir.exists(), "boot created the draft directory");
    assert!(loader.last_error().is_none());

    loader.dispose().unwrap();
    assert!(!dir.exists());
}

#[test]
fn with_document_wins_over_initial_and_suppresses_its_write() {
    let path = temp_path("precedence");
    let starts = Arc::new(AtomicUsize::new(0));
    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&path)
            .with_registry(counting_registry(&starts))
            .with_document(Document::with_entries(vec![
                EntryOptions::new("worker").with_id("from-document"),
            ]))
            .with_initial(Document::with_entries(vec![
                EntryOptions::new("worker").with_id("from-initial"),
            ])),
    )
    .unwrap();

    assert!(loader.tree().resolve("from-document").is_some());
    assert!(loader.tree().resolve("from-initial").is_none());
    assert!(
        !path.exists(),
        "initial write must not happen for documents"
    );

    loader.dispose().unwrap();
    cleanup(&path);
}

#[test]
fn write_back_lands_in_the_draft_file() {
    let path = temp_path("writeback");
    let starts = Arc::new(AtomicUsize::new(0));
    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&path)
            .with_registry(counting_registry(&starts))
            .with_document(Document::with_entries(vec![
                EntryOptions::new("worker").with_id("w1"),
            ])),
    )
    .unwrap();

    // update_config is the write-back moment: the draft is created holding
    // the composed tree with the new config.
    loader
        .update_config("w1", Node::from_iter([("port".to_string(), Node::Int(2))]))
        .unwrap();
    assert!(path.exists(), "write-back must land in the draft");
    let reread = loader.file().read().unwrap();
    assert_eq!(reread.entries.len(), 1);
    assert_eq!(reread.entries[0].id.as_deref(), Some("w1"));
    assert_eq!(
        reread.entries[0].config,
        Some(Node::from_iter([("port".to_string(), Node::Int(2))]))
    );

    loader.dispose().unwrap();
    cleanup(&path);
}

#[test]
fn document_backed_reload_ignores_the_draft_file() {
    let path = temp_path("draft-ignore");
    let starts = Arc::new(AtomicUsize::new(0));
    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&path)
            .with_registry(counting_registry(&starts))
            .with_document(Document::with_entries(vec![
                EntryOptions::new("worker").with_id("w1"),
                EntryOptions::new("worker").with_id("w2"),
            ])),
    )
    .unwrap();

    // Pollute the draft exactly the way a loader write-back would: composed
    // rows (and a disable) baked into the root file must never re-enter the
    // composition or affect the mounted tree.
    let draft = "entries:\n  - id: rogue\n    name: worker\n  - id: w1\n    name: worker\n    disabled: true\n";
    std::fs::write(&path, draft).unwrap();

    let diff = loader.reload().unwrap();
    assert!(diff.created.is_empty() && diff.removed.is_empty());
    assert!(
        loader.tree().resolve("rogue").is_none(),
        "draft row mounted"
    );
    let w1 = loader.tree().resolve("w1").unwrap();
    assert!(w1.fiber().is_some(), "draft disable leaked into the tree");
    assert!(!w1.options().disabled.is_disabled());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        draft,
        "reload rewrote the draft"
    );

    loader.dispose().unwrap();
    cleanup(&path);
}

#[test]
fn update_reconciles_without_write_back() {
    let path = temp_path("update");
    let starts = Arc::new(AtomicUsize::new(0));
    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&path)
            .with_registry(counting_registry(&starts))
            .with_document(Document::with_entries(vec![
                EntryOptions::new("worker")
                    .with_id("w2")
                    .with_config(Node::from_iter([("port".to_string(), Node::Int(1))])),
                EntryOptions::new("group")
                    .with_id("g")
                    .with_group(vec![EntryOptions::new("worker").with_id("w1")]),
            ])),
    )
    .unwrap();
    let w2_before = loader.tree().resolve("w2").unwrap().fiber().unwrap();

    // Recomposition: w2 config-only (patch in place), w3 and an id-less row
    // created, the g/w1 subtree removed. The id-less row makes the pass
    // dirty, but a recomposition is not a file edit — nothing is written.
    let diff = loader
        .recompose(Document::with_entries(vec![
            EntryOptions::new("worker")
                .with_id("w2")
                .with_config(Node::from_iter([("port".to_string(), Node::Int(2))])),
            EntryOptions::new("worker").with_id("w3"),
            EntryOptions::new("worker"),
        ]))
        .unwrap();

    assert!(diff.updated.iter().any(|entry| entry.id() == "w2"));
    assert_eq!(diff.created.len(), 2, "w3 and the id-less row");
    assert!(diff.removed.iter().any(|removed| removed.entry.id() == "g"));
    // Config-only change: same fiber, patched in place.
    let w2 = loader.tree().resolve("w2").unwrap();
    assert!(w2.fiber().unwrap().ptr_eq(&w2_before));
    for id in ["w2", "w3"] {
        loader
            .tree()
            .resolve(id)
            .unwrap()
            .fiber()
            .unwrap()
            .try_wait()
            .unwrap();
    }
    // Two starts at open (w1, w2), two more from the update (w3, id-less),
    // and one apply re-run for w2's in-place patch (same fiber — the ptr_eq
    // above) — not a stop-and-start cycle, which would need no extra apply.
    assert_eq!(starts.load(Ordering::SeqCst), 5);
    assert!(loader.tree().resolve("w1").is_none());
    assert!(
        !path.exists(),
        "dirty rows must stay in memory — no write-back"
    );

    loader.dispose().unwrap();
    cleanup(&path);
}

#[test]
fn update_replaces_the_composition_source_for_later_reloads() {
    let path = temp_path("source");
    let starts = Arc::new(AtomicUsize::new(0));
    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&path)
            .with_registry(counting_registry(&starts))
            .with_initial(Document::with_entries(vec![
                EntryOptions::new("worker").with_id("w1"),
            ])),
    )
    .unwrap();
    assert!(path.exists(), "file-backed boot still writes initial");

    // update() makes the document the composition source: a later reload
    // recomposes from it instead of re-reading the file (which still says w1).
    loader
        .recompose(Document::with_entries(vec![
            EntryOptions::new("worker").with_id("w2"),
        ]))
        .unwrap();
    loader.reload().unwrap();
    assert!(loader.tree().resolve("w1").is_none());
    let w2 = loader.tree().resolve("w2").unwrap();
    w2.fiber().unwrap().try_wait().unwrap();

    loader.dispose().unwrap();
    cleanup(&path);
}

#[test]
fn update_mounts_import_rows() {
    let path = temp_path("import-row");
    let services = path.with_file_name("services.yml");
    std::fs::write(&services, "entries:\n  - id: s1\n    name: worker\n").unwrap();
    let starts = Arc::new(AtomicUsize::new(0));
    let root = Context::new();
    let loader = Loader::open(
        &root,
        LoaderConfig::new(&path)
            .with_registry(counting_registry(&starts))
            .with_document(Document::with_entries(vec![
                EntryOptions::new("worker").with_id("m1"),
            ])),
    )
    .unwrap();

    // A composition can insert an import row; its file mounts as a subtree.
    loader
        .recompose(Document::with_entries(vec![
            EntryOptions::new("worker").with_id("m1"),
            EntryOptions::new("import")
                .with_id("imp")
                .with_config(Node::from_iter([(
                    "url".to_string(),
                    "services.yml".into(),
                )])),
        ]))
        .unwrap();
    let s1 = loader.tree().resolve("imp:s1").unwrap();
    s1.fiber().unwrap().try_wait().unwrap();
    assert_eq!(starts.load(Ordering::SeqCst), 2);
    assert!(loader.last_error().is_none());

    loader.dispose().unwrap();
    cleanup(&path);
}

#[test]
fn concurrent_document_boots_never_write_the_shared_draft() {
    // The naive "compose → write draft → open" races between concurrent
    // boots: another process's draft can land between this one's write and
    // read, and this boot mounts a composition it never made. Document
    // boots never read or write the draft, so every loader sees exactly its
    // own document.
    let path = temp_path("race");
    let handles: Vec<_> = (0..8)
        .map(|index| {
            let path = path.clone();
            std::thread::spawn(move || {
                let starts = Arc::new(AtomicUsize::new(0));
                let root = Context::new();
                let loader = Loader::open(
                    &root,
                    LoaderConfig::new(&path)
                        .with_registry(counting_registry(&starts))
                        .with_document(Document::with_entries(vec![
                            EntryOptions::new("worker").with_id(format!("w{index}")),
                        ])),
                )
                .unwrap();
                let own = format!("w{index}");
                let entry = loader.tree().resolve(&own).unwrap();
                entry.fiber().unwrap().try_wait().unwrap();
                assert_eq!(loader.tree().entries().len(), 1, "mounted a foreign row");
                assert!(!path.exists(), "a boot wrote the shared draft");
                loader.dispose().unwrap();
            })
        })
        .collect();
    for handle in handles {
        handle.join().expect("concurrent boot thread");
    }
    assert!(!path.exists());
    cleanup(&path);
}

#[test]
fn recomposition_drops_a_self_kill_disable_written_to_the_draft() {
    let path = temp_path("selfkill-drop");
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
            .with_document(Document::with_entries(vec![
                EntryOptions::new("victim").with_id("v1"),
            ])),
    )
    .unwrap();
    let entry = loader.tree().resolve("v1").unwrap();
    entry.fiber().unwrap().try_wait().unwrap();

    // The plugin tears itself down: the disable persists into the draft
    // (write-back), deferred off the dying fiber's transition lock.
    victim_fiber
        .lock()
        .unwrap()
        .clone()
        .unwrap()
        .dispose()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if entry.fiber().is_none()
            && std::fs::read_to_string(&path)
                .unwrap_or_default()
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

    // A fresh recomposition deliberately drops the disable: the patch
    // composition tree is short-lived, and the draft is never an input.
    loader
        .recompose(Document::with_entries(vec![
            EntryOptions::new("victim").with_id("v1"),
        ]))
        .unwrap();
    let revived = loader.tree().resolve("v1").unwrap();
    revived.fiber().unwrap().try_wait().unwrap();
    assert!(!revived.options().disabled.is_disabled());

    loader.dispose().unwrap();
    cleanup(&path);
}
