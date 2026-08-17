//! Loader file round-trips, atomic writes, suspension, and interpolation.

use cordis_include::{Document, EntryOptions, EntryTree, IncludeError, LoaderFile, Node};
use std::path::PathBuf;
use std::time::Duration;

fn temp_path(stem: &str, ext: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "cordis-include-test-{stem}-{}.{ext}",
        std::process::id()
    ))
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let _ = std::fs::remove_file(path.with_file_name(format!("{name}.tmp")));
}

#[test]
fn yaml_round_trip_is_stable_and_ordered() {
    let path = temp_path("roundtrip", "yml");
    cleanup(&path);
    let file = LoaderFile::open(&path).unwrap();
    assert_eq!(file.format(), cordis_include::FileFormat::Yaml);

    let document = Document::with_entries(vec![
        EntryOptions::new("adapter")
            .with_id("abc123")
            .with_config(Node::from_iter([
                ("host".to_string(), "localhost".into()),
                ("port".to_string(), 8080.into()),
            ])),
        EntryOptions::new("group").with_id("g").with_group(vec![
            EntryOptions::new("child").with_id("c").with_disabled(true),
        ]),
    ]);
    file.write(&document).unwrap();
    let reread = file.read().unwrap();
    assert_eq!(reread, document);

    // Field order: id/name first, config last — stable across rewrites.
    let text = std::fs::read_to_string(&path).unwrap();
    let name_pos = text.find("name: adapter").expect("name present");
    let config_pos = text.find("config:").expect("config present");
    assert!(name_pos < config_pos, "{text}");
    file.write(&reread).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        text,
        "rewrite is byte-stable"
    );

    // No temp file is left behind.
    assert!(
        !path
            .with_file_name(format!(
                "{}.tmp",
                path.file_name().unwrap().to_string_lossy()
            ))
            .exists()
    );
    cleanup(&path);
}

#[test]
fn json_round_trip() {
    let path = temp_path("roundtrip", "json");
    cleanup(&path);
    let file = LoaderFile::open(&path).unwrap();
    let document = Document::with_entries(vec![
        EntryOptions::new("a")
            .with_id("x")
            .with_config(Node::from_iter([("n".to_string(), 1.into())])),
    ]);
    file.write(&document).unwrap();
    assert_eq!(file.read().unwrap(), document);
    assert!(std::fs::read_to_string(&path).unwrap().starts_with('{'));
    cleanup(&path);
}

#[test]
fn unknown_extensions_are_rejected() {
    assert!(matches!(
        LoaderFile::open("config.toml"),
        Err(IncludeError::UnknownFormat { .. })
    ));
}

#[test]
fn missing_and_empty_files_read_as_empty_documents() {
    let path = temp_path("missing", "yml");
    cleanup(&path);
    let file = LoaderFile::open(&path).unwrap();
    assert_eq!(file.read().unwrap(), Document::default());

    std::fs::write(&path, "").unwrap();
    assert_eq!(file.read().unwrap(), Document::default());
    std::fs::write(&path, "null\n").unwrap();
    assert_eq!(file.read().unwrap(), Document::default());
    cleanup(&path);
}

#[test]
fn unknown_top_level_keys_are_preserved() {
    let path = temp_path("extra", "yml");
    cleanup(&path);
    std::fs::write(
        &path,
        "version: 3\nentries:\n  - name: a\n    id: k\nauthor: someone\n",
    )
    .unwrap();
    let file = LoaderFile::open(&path).unwrap();
    let mut document = file.read().unwrap();
    assert_eq!(document.extra["version"], Node::Int(3));
    assert_eq!(document.extra["author"], "someone".into());

    // A tree reload keeps the extras while entries churn.
    let tree = EntryTree::new();
    tree.update(document.entries.clone()).unwrap();
    document.entries = tree.serialize();
    file.write(&document).unwrap();
    let reread = file.read().unwrap();
    assert_eq!(reread.extra["version"], Node::Int(3));
    assert!(
        reread
            .entries
            .iter()
            .any(|options| options.id.as_deref() == Some("k"))
    );
    cleanup(&path);
}

#[test]
fn suspended_writes_are_no_ops() {
    let path = temp_path("suspend", "yml");
    cleanup(&path);
    let file = LoaderFile::open(&path).unwrap();
    file.write(&Document::with_entries(vec![EntryOptions::new("a")]))
        .unwrap();
    let before = std::fs::read_to_string(&path).unwrap();

    let guard = file.suspend();
    assert!(file.is_suspended());
    file.write(&Document::with_entries(vec![EntryOptions::new("b")]))
        .unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    drop(guard);
    assert!(!file.is_suspended());

    file.write(&Document::with_entries(vec![EntryOptions::new("b")]))
        .unwrap();
    assert_ne!(std::fs::read_to_string(&path).unwrap(), before);
    cleanup(&path);
}

#[cfg(unix)]
#[test]
fn readonly_files_refuse_writes() {
    use std::os::unix::fs::PermissionsExt;

    let path = temp_path("readonly", "yml");
    cleanup(&path);
    let file = LoaderFile::open(&path).unwrap();
    file.write(&Document::default()).unwrap();

    let read_only = std::fs::Permissions::from_mode(0o444);
    std::fs::set_permissions(&path, read_only).unwrap();

    // Skip the assertion when the process can write anyway (e.g. root).
    if std::fs::metadata(&path).unwrap().permissions().readonly() {
        assert!(matches!(
            file.write(&Document::default()),
            Err(IncludeError::ReadOnly { .. })
        ));
    }

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    cleanup(&path);
}

#[test]
fn write_creates_missing_parent_directories() {
    let path = std::env::temp_dir().join(format!(
        "cordis-include-test-nested-{}/config.yml",
        std::process::id()
    ));
    cleanup(&path);
    let file = LoaderFile::open(&path).unwrap();
    file.write(&Document::with_entries(vec![EntryOptions::new("a")]))
        .unwrap();
    assert!(path.exists());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn interpolation_expands_on_read_but_preserves_the_file() {
    let path = temp_path("interpolate", "yml");
    cleanup(&path);
    let name = format!("CORDIS_INCLUDE_TEST_VAR_{}", std::process::id());
    std::fs::write(
        &path,
        format!(
            "entries:\n  - name: a\n    id: k\n    config:\n      host: ${{{{ env.{name} }}}}\n"
        ),
    )
    .unwrap();
    // `set_var` is `unsafe` in edition 2024; the variable is unique to this
    // process and only touched by this test.
    unsafe { std::env::set_var(&name, "example.org") };

    let file = LoaderFile::open(&path).unwrap();
    let tree = EntryTree::new();
    tree.update(file.read().unwrap().entries).unwrap();
    let entry = tree.resolve("k").unwrap();

    // Raw config keeps the template; resolved config substitutes it.
    assert_eq!(
        entry.config().unwrap()["host"].as_str().unwrap(),
        format!("${{{{ env.{name} }}}}")
    );
    let resolved = entry.resolved_config().unwrap().unwrap();
    assert_eq!(resolved["host"].as_str(), Some("example.org"));

    // Writing back serializes the raw template, not the expansion.
    let mut document = file.read().unwrap();
    document.entries = tree.serialize();
    file.write(&document).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains(&format!("${{{{ env.{name} }}}}")), "{text}");
    unsafe { std::env::remove_var(&name) };
    cleanup(&path);
}

#[test]
fn parse_errors_surface_the_format() {
    let path = temp_path("broken", "yml");
    cleanup(&path);
    std::fs::write(&path, "entries: [unclosed").unwrap();
    let file = LoaderFile::open(&path).unwrap();
    assert!(matches!(
        file.read(),
        Err(IncludeError::Parse { format: "yaml", .. })
    ));
    cleanup(&path);
}

#[test]
fn deferred_writes_coalesce_latest_wins() {
    let path = temp_path("deferred", "yml");
    cleanup(&path);
    let file = LoaderFile::open(&path).unwrap();
    let stale = Document::with_entries(vec![EntryOptions::new("a")]);
    let latest = Document::with_entries(vec![EntryOptions::new("b"), EntryOptions::new("c")]);

    file.write_deferred(stale, Duration::from_millis(300));
    std::thread::sleep(Duration::from_millis(50));
    // The second call replaces the pending document and resets the deadline.
    file.write_deferred(latest.clone(), Duration::from_millis(300));
    file.flush_deferred();

    assert_eq!(file.read().unwrap(), latest);
    assert!(file.last_deferred_error().is_none());
    cleanup(&path);
}

#[test]
fn deferred_writes_wait_for_suspension_to_lift() {
    let path = temp_path("deferred-suspend", "yml");
    cleanup(&path);
    let file = LoaderFile::open(&path).unwrap();
    let document = Document::with_entries(vec![EntryOptions::new("a")]);

    let guard = file.suspend();
    file.write_deferred(document.clone(), Duration::from_millis(50));
    let flushing = file.clone();
    let flusher = std::thread::spawn(move || flushing.flush_deferred());

    // While suspended nothing lands; the flusher is parked behind the guard.
    std::thread::sleep(Duration::from_millis(300));
    assert!(!path.exists(), "suspension defers the physical write");
    drop(guard);
    flusher.join().unwrap();
    assert_eq!(file.read().unwrap(), document);
    cleanup(&path);
}
