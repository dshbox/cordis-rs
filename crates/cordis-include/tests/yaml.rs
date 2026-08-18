//! The YAML dialect through the crate's public IO: `!!js` expressions in
//! config files, patch files, and dumps — parse, round-trip, and emit.

use cordis_include::{
    Disabled, Document, DumpLayer, EntryOptions, LoaderFile, Node, PatchOptions,
    load_overlay_patches, render_config_dump,
};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_path(stem: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "cordis-include-yaml-test-{stem}-{}-{nanos}.yml",
        std::process::id()
    ))
}

#[test]
fn loader_files_round_trip_js_expressions_in_config() {
    let path = temp_path("expr-roundtrip");
    let _ = std::fs::remove_file(&path);
    std::fs::write(
        &path,
        "entries:\n  - id: llm\n    name: adapter-llm\n    config:\n      model: !!js process.env.DSH_MODEL || 'default'\n      retries: 3\n",
    )
    .unwrap();
    let file = LoaderFile::open(&path).unwrap();
    let document = file.read().unwrap();
    let config = document.entries[0].config.as_ref().unwrap();
    assert_eq!(
        config.as_object().unwrap()["model"],
        Node::Expr("process.env.DSH_MODEL || 'default'".to_owned())
    );
    assert_eq!(config.as_object().unwrap()["retries"], Node::Int(3));

    // Write-back emits the expression verbatim, and the rewrite is stable.
    file.write(&document).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("model: !!js process.env.DSH_MODEL || 'default'\n"),
        "{text}"
    );
    file.write(&document).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn empty_entries_tail_reads_as_an_empty_document() {
    let path = temp_path("entries-tail");
    let _ = std::fs::remove_file(&path);
    std::fs::write(&path, "entries:\n").unwrap();
    let file = LoaderFile::open(&path).unwrap();
    assert_eq!(file.read().unwrap(), Document::default());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn entry_files_round_trip_disabled_expressions() {
    let path = temp_path("disabled-expr");
    let _ = std::fs::remove_file(&path);
    std::fs::write(
        &path,
        "entries:\n  - id: gate\n    name: adapter\n    disabled: !!js process.platform === 'win32'\n",
    )
    .unwrap();
    let file = LoaderFile::open(&path).unwrap();
    let document = file.read().unwrap();
    assert_eq!(
        document.entries[0].disabled,
        Disabled::Expr("process.platform === 'win32'".to_owned())
    );
    file.write(&document).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("disabled: !!js process.platform === 'win32'\n"),
        "{text}"
    );
    // Write-back is stable and the expression survives re-reading.
    file.write(&document).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    assert_eq!(file.read().unwrap(), document);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn patch_files_keep_expressions_and_reject_them_for_disabled() {
    let path = temp_path("patch-expr");
    let _ = std::fs::remove_file(&path);
    std::fs::write(
        &path,
        "\
- id: agent-loop
  config:
    model: !!js process.env.DSH_SPEC_MODEL
- insert:
    - id: llm
      name: adapter-llm
",
    )
    .unwrap();
    let patches = load_overlay_patches(&path).unwrap();
    assert_eq!(patches.len(), 2);
    let config = patches[0].config.as_ref().unwrap();
    assert_eq!(
        config.as_object().unwrap()["model"],
        Node::Expr("process.env.DSH_SPEC_MODEL".to_owned())
    );
    assert_eq!(
        patches[1].insert.as_ref().unwrap()[0].id.as_deref(),
        Some("llm")
    );
    let _ = std::fs::remove_file(&path);

    // Patch overrides need an evaluated boolean; entry rows carry the
    // expression slot instead.
    std::fs::write(
        &path,
        "- id: x\n  disabled: !!js process.platform === 'win32'\n",
    )
    .unwrap();
    let error = load_overlay_patches(&path).unwrap_err().to_string();
    assert!(
        error.contains("disabled: !!js expressions are not supported in patches"),
        "{error}"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn dumps_print_expressions_unevaluated() {
    let base = [EntryOptions::new("adapter").with_id("shared")];
    let overlay = [PatchOptions {
        id: Some("shared".into()),
        config: Some(Node::from_iter([(
            "model".to_string(),
            Node::Expr("process.env.DSH_MODEL".to_owned()),
        )])),
        ..Default::default()
    }];
    let layers = [DumpLayer {
        label: "overlay.yml",
        patches: &overlay,
    }];
    let dump = render_config_dump(&base, "base.yml", &layers, |_| {}).unwrap();
    assert!(
        dump.contains("# == base.yml, patched by overlay.yml"),
        "{dump}"
    );
    assert!(
        dump.contains("model: !!js process.env.DSH_MODEL\n"),
        "{dump}"
    );
    // The dump stays loadable and the expression survives a re-parse.
    let entries = cordis_include::parse_entry_list(&dump).unwrap();
    let config = entries[0].config.as_ref().unwrap();
    assert_eq!(
        config.as_object().unwrap()["model"],
        Node::Expr("process.env.DSH_MODEL".to_owned())
    );
}

/// A `disabled: !!js` slot dumps unevaluated, like the config
/// expressions — upstream's `--dump-config` never evaluates `!!js`.
#[test]
fn dumps_print_disabled_expressions_unevaluated() {
    let base = [EntryOptions {
        id: Some("gate".into()),
        name: "adapter".into(),
        disabled: Disabled::Expr("process.env.DSH_GATE === 'off'".to_owned()),
        ..EntryOptions::default()
    }];
    let dump = render_config_dump(&base, "base.yml", &[], |_| {}).unwrap();
    assert!(
        dump.contains("disabled: !!js process.env.DSH_GATE === 'off'\n"),
        "{dump}"
    );
    // The dump is loadable and the slot round-trips to the expression.
    let entries = cordis_include::parse_entry_list(&dump).unwrap();
    assert_eq!(
        entries[0].disabled,
        Disabled::Expr("process.env.DSH_GATE === 'off'".to_owned())
    );
}
