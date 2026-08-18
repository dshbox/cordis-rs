//! Patch algebra: apply semantics, patch-file IO fail-loud contracts, layer
//! composition (single flatten), provenance, and dump rendering — the port
//! of upstream's include patch machinery and its app-boot test face.

use cordis_include::{
    DumpLayer, EntryOptions, IncludeError, Node, PatchOptions, Provenance, apply_entry_patches,
    compose_layers, compose_with_provenance, load_optional_patches, load_overlay_patches,
    render_config_dump, render_dump,
};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

/// A warning sink collectable after the call.
fn warning_sink() -> (Rc<std::cell::RefCell<Vec<String>>>, impl FnMut(&str)) {
    let lines = Rc::new(std::cell::RefCell::new(Vec::new()));
    let sink = {
        let lines = lines.clone();
        move |line: &str| lines.borrow_mut().push(line.to_owned())
    };
    (lines, sink)
}

/// Parse a patch list from YAML text (the file shape, without the file).
fn patches(text: &str) -> Vec<PatchOptions> {
    serde_yaml_ng::from_str(text).expect("valid patch list")
}

/// An entry with id and name.
fn entry(id: &str, name: &str) -> EntryOptions {
    EntryOptions::new(name).with_id(id)
}

/// The object config of a patch row.
fn patch_config_of(patch: &PatchOptions) -> &cordis_include::NodeMap {
    patch
        .config
        .as_ref()
        .and_then(Node::as_object)
        .expect("config object")
}

/// A unique temp file path for patch-list IO tests.
fn temp_path(stem: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "cordis-include-patch-test-{stem}-{}-{nanos}.yml",
        std::process::id()
    ))
}

fn config_of(entry: &EntryOptions) -> &cordis_include::NodeMap {
    entry
        .config
        .as_ref()
        .and_then(Node::as_object)
        .expect("config object")
}

// ---------------------------------------------------------------- apply ---

#[test]
fn overrides_replace_config_wholesale_and_flip_flags() {
    let base = vec![entry("a", "adapter").with_config(Node::from_iter([
        ("v".to_string(), Node::Int(1)),
        ("keep".to_string(), Node::Bool(true)),
    ]))];
    let composed = apply_entry_patches(
        &base,
        &patches(
            "\
- id: a
  config:
    v: 2
- id: a
  disabled: true
- id: a
  inject: [database]
",
        ),
        |_| {},
    );
    assert_eq!(composed.len(), 1);
    // `config` is a whole replacement, not a merge.
    assert_eq!(
        composed[0].config,
        Some(Node::from_iter([("v".to_string(), Node::Int(2))]))
    );
    assert!(composed[0].disabled.is_disabled());
    assert_eq!(composed[0].inject, ["database"]);
}

#[test]
fn insert_without_id_appends_to_top_level_and_is_immediately_indexed() {
    let composed = apply_entry_patches(
        &[],
        &patches(
            "\
- insert:
    - id: x
      name: pkg-x
      config:
        a: 1
- id: x
  config:
    a: 2
",
        ),
        |_| {},
    );
    // A later patch in the same list targets the row the earlier one
    // inserted — without immediate indexing, inserted rows are unpatchable.
    assert_eq!(
        composed,
        vec![entry("x", "pkg-x").with_config(Node::from_iter([("a".to_string(), Node::Int(2))]))]
    );
}

#[test]
fn insert_into_group_children_and_indexing_recurses_into_them() {
    let base = vec![entry("g", "group")];
    let composed = apply_entry_patches(
        &base,
        &patches(
            "\
- id: g
  insert:
    - id: child
      name: pkg-c
      group:
        - id: grandchild
          name: pkg-gc
- id: grandchild
  disabled: true
",
        ),
        |_| {},
    );
    let child = &composed[0].group[0];
    assert_eq!(child.id.as_deref(), Some("child"));
    // The inserted row's own group children are indexed too.
    assert!(child.group[0].disabled.is_disabled());
}

#[test]
fn insert_with_id_requires_a_group_target() {
    let base = vec![
        entry("g", "group").with_group(vec![entry("c0", "pkg")]),
        // Declared group with no rows yet — still an insert target.
        entry("empty", "group"),
        entry("plain", "adapter"),
    ];
    let (lines, sink) = warning_sink();
    let composed = apply_entry_patches(
        &base,
        &patches(
            "\
- id: g
  insert:
    - id: c1
      name: pkg
- id: empty
  insert:
    - id: e1
      name: pkg
- id: plain
  insert:
    - id: p1
      name: pkg
- id: absent
  insert:
    - id: z1
      name: pkg
",
        ),
        sink,
    );
    assert_eq!(composed[0].group.len(), 2);
    assert_eq!(
        composed[1].group.len(),
        1,
        "childless group accepts inserts"
    );
    assert!(composed[2].group.is_empty(), "non-group rejects inserts");
    assert_eq!(
        lines.borrow().to_vec(),
        [
            "patch insert: entry \"plain\" is not a group",
            "patch insert: entry \"absent\" not found",
        ],
    );
}

#[test]
fn name_is_a_guard_not_an_override() {
    let base = vec![entry("a", "adapter")];
    let (lines, sink) = warning_sink();
    let composed = apply_entry_patches(
        &base,
        &patches(
            "\
- id: a
  name: adapter
  disabled: true
- id: a
  name: other-plugin
  inject: [wrong]
- id: a
  name: ''
  inject: [empty-guard]
",
        ),
        sink,
    );
    // Matching guard applies; mismatched guard warns and skips; an empty
    // guard string is no guard at all (upstream truthiness).
    assert!(composed[0].disabled.is_disabled());
    assert_eq!(composed[0].inject, ["empty-guard"]);
    assert_eq!(composed[0].name, "adapter");
    assert_eq!(
        lines.borrow().to_vec(),
        ["patch: name mismatch for \"a\" (expected \"adapter\", got \"other-plugin\"), skipping"],
    );
}

#[test]
fn misses_and_missing_ids_warn_and_skip() {
    let (lines, sink) = warning_sink();
    let composed = apply_entry_patches(
        &[entry("a", "adapter")],
        &patches(
            "\
- config: {}
- id: missing
  config: {}
",
        ),
        sink,
    );
    assert_eq!(composed, vec![entry("a", "adapter")]);
    assert_eq!(
        lines.borrow().to_vec(),
        [
            "patch: id is required for non-insert patches",
            "patch: entry \"missing\" not found",
        ],
    );
}

#[test]
fn group_and_unknown_override_keys_warn_and_skip() {
    let base = vec![entry("a", "adapter").with_group(vec![entry("keep", "pkg")])];
    let (lines, sink) = warning_sink();
    let composed = apply_entry_patches(
        &base,
        &patches(
            "\
- id: a
  group:
    - id: injected
      name: pkg
- id: a
  intercept: database
  isolate: true
",
        ),
        sink,
    );
    // `group` is the children list here, not upstream's marker; overriding
    // it is not equivalent, so it (and any unknown key) warns and skips.
    assert_eq!(composed[0].group, vec![entry("keep", "pkg")]);
    assert_eq!(
        lines.borrow().to_vec(),
        [
            "patch: skipping unsupported override key(s) [group] on entry \"a\"",
            "patch: skipping unsupported override key(s) [intercept, isolate] on entry \"a\"",
        ],
    );
}

#[test]
fn insert_rows_without_ids_warn() {
    let (lines, sink) = warning_sink();
    let composed = apply_entry_patches(
        &[],
        &patches(
            "\
- insert:
    - name: no-id
      group:
        - name: nested-no-id
",
        ),
        sink,
    );
    assert_eq!(composed.len(), 1);
    assert_eq!(composed[0].group.len(), 1);
    let lines = lines.borrow().to_vec();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].contains("has no id"), "{lines:?}");
    assert!(lines[1].contains("has no id"), "{lines:?}");
}

#[test]
fn empty_insert_list_still_takes_the_insert_branch() {
    // Upstream truthiness: `insert: []` is an insert; on a non-group target
    // it still warns (and does nothing), rather than falling through to the
    // override branch.
    let (lines, sink) = warning_sink();
    let composed = apply_entry_patches(
        &[entry("plain", "adapter")],
        &patches("- id: plain\n  insert: []\n"),
        sink,
    );
    assert_eq!(composed, vec![entry("plain", "adapter")]);
    assert_eq!(
        lines.borrow().to_vec(),
        ["patch insert: entry \"plain\" is not a group"],
    );
}

#[test]
fn empty_patch_list_returns_a_detached_copy() {
    let base = vec![entry("a", "adapter")];
    let mut composed = apply_entry_patches(&base, &[], |_| {});
    assert_eq!(composed, base);
    // The result shares nothing with the input: mutating it cannot leak
    // back into the base a later recomposition would start from.
    composed[0].name = "changed".into();
    assert_eq!(base[0].name, "adapter");
}

#[test]
fn recomposition_from_original_data_reverts_removed_patches() {
    // The detachment contract: patching or mounting shared entry objects
    // would bake earlier values into the cached parse, so a removed or
    // changed patch could never be reverted. Inputs are immutable here —
    // pin that recomposition from the same originals stays exact.
    let base = vec![entry("a", "adapter")];
    let layer_a = patches("- id: a\n  disabled: true\n");
    let layer_b = patches("- id: a\n  config:\n    v: 1\n");

    let with_both =
        apply_entry_patches(&base, &[layer_a.clone(), layer_b.clone()].concat(), |_| {});
    assert!(with_both[0].disabled.is_disabled());
    assert_eq!(
        with_both[0].config,
        Some(Node::from_iter([("v".to_string(), Node::Int(1))]))
    );

    let with_a_only = apply_entry_patches(&base, &layer_a, |_| {});
    assert!(
        with_a_only[0].disabled.is_disabled(),
        "removing layer b reverts only b"
    );
    assert!(with_a_only[0].config.is_none(), "layer b's write is gone");

    // Inputs were never touched: the patch rows still hold their inserts.
    assert!(layer_b[0].config.is_some());
    assert!(base[0].config.is_none());
}

// -------------------------------------------------------------- compose ---

#[test]
fn compose_layers_applies_over_an_empty_root_and_reports_skips() {
    let (lines, sink) = warning_sink();
    let entries = compose_layers(
        &[
            patches("- insert:\n    - id: x\n      name: pkg-x\n      config:\n        a: 1\n"),
            patches("- id: x\n  config:\n    a: 2\n- id: missing\n  config: {}\n"),
        ],
        sink,
    );
    assert_eq!(
        entries,
        vec![entry("x", "pkg-x").with_config(Node::from_iter([("a".to_string(), Node::Int(2))]))]
    );
    let lines = lines.borrow().to_vec();
    assert!(
        lines.iter().any(|line| line.contains("\"missing\"")),
        "{lines:?}"
    );
}

#[test]
fn single_flatten_never_sees_rows_a_config_replacement_introduced() {
    // The corner case that makes single-flattening load-bearing upstream: a
    // group's `config` replacement can introduce entry-shaped rows, and the
    // single-pass id index never sees them, so a later layer targeting such
    // a row warns and misses. Composing layer-by-layer would rebuild the
    // index between layers and patch a row a single-call composition (and
    // therefore a boot) never touches. Here the port's mapping: upstream
    // stores group children in `config`; this crate stores plugin config
    // there and children in `group`, so the introduction happens through a
    // plain config replacement all the same.
    let base = vec![entry("g", "group")];
    let layer_a = patches(
        "\
- id: g
  config:
    - id: child
      name: pkg
      config:
        v: 1
",
    );
    let layer_b = patches("- id: child\n  config:\n    v: 2\n");

    let (lines, sink) = warning_sink();
    let entries = apply_entry_patches(&base, &[layer_a, layer_b].concat(), sink);
    assert_eq!(
        lines.borrow().to_vec(),
        ["patch: entry \"child\" not found"],
    );
    // The skipped layer did not touch the row it could not see.
    let introduced = entries[0].config.as_ref().and_then(Node::as_array).unwrap();
    let child = introduced[0].as_object().unwrap();
    assert_eq!(child["config"]["v"], Node::Int(1));
}

#[test]
fn layers_patch_rows_earlier_layers_inserted() {
    // The positive control for the index's reach: inserted rows (and rows
    // inserted into groups) are indexed immediately.
    let entries = compose_layers(
        &[
            patches("- insert:\n    - id: x\n      name: pkg\n"),
            patches("- id: x\n  disabled: true\n"),
        ],
        |_| {},
    );
    assert_eq!(entries.len(), 1);
    assert!(entries[0].disabled.is_disabled());
}

// ------------------------------------------------------------------ IO ---

#[test]
fn optional_patches_treat_a_missing_file_as_no_layer() {
    assert_eq!(
        load_optional_patches(temp_path("absent")).unwrap(),
        None,
        "missing file means no layer"
    );
}

#[test]
fn optional_patches_parse_lists_and_preserve_unknown_keys() {
    let path = temp_path("parse");
    std::fs::write(
        &path,
        "\
- id: agent-loop
  config:
    model: ${{ env.MODEL }}
- insert:
    - id: llm
      name: pkg-llm
- id: odd
  intercept: database
",
    )
    .unwrap();
    let patches = load_optional_patches(&path)
        .unwrap()
        .expect("present layer");
    assert_eq!(patches.len(), 3);
    assert_eq!(patches[0].id.as_deref(), Some("agent-loop"));
    // Templates stay literal — they belong to the entry's own evaluation.
    assert_eq!(
        patch_config_of(&patches[0])["model"],
        "${{ env.MODEL }}".into()
    );
    assert_eq!(
        patches[1].insert.as_ref().unwrap()[0].id.as_deref(),
        Some("llm")
    );
    assert_eq!(
        patches[2].extra.get("intercept"),
        Some(&Node::String("database".to_string()))
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn optional_patches_fail_loud_on_every_breakage() {
    // A present patch file that cannot apply is a misconfiguration and must
    // fail loud — never be silently skipped.
    let dir = temp_path("unreadable");
    std::fs::create_dir_all(&dir).unwrap(); // a directory: present, unreadable as a file
    assert!(matches!(
        load_optional_patches(&dir),
        Err(IncludeError::Message { .. })
    ));
    let message = load_optional_patches(&dir).unwrap_err().to_string();
    assert!(message.starts_with("failed to read patches "), "{message}");

    let cases = [
        ("invalid: [unclosed\n", "failed to parse patches "),
        (
            "id: not-a-list\n",
            "must be a top-level YAML array of loader patch entries",
        ),
        (
            "- just-a-string\n",
            "must be a mapping (a loader patch entry)",
        ),
        (
            "- id: x\n  disabled: not-a-bool\n",
            "failed to parse patches entry 1 in ",
        ),
    ];
    for (index, (text, expected)) in cases.iter().enumerate() {
        let path = temp_path(&format!("broken-{index}"));
        std::fs::write(&path, text).unwrap();
        let error = load_optional_patches(&path).unwrap_err().to_string();
        assert!(error.contains(expected), "case {index}: {error}");
        let _ = std::fs::remove_file(&path);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn overlay_patches_fail_loud_when_missing_and_parse_when_present() {
    let error = load_overlay_patches(temp_path("overlay-absent"))
        .unwrap_err()
        .to_string();
    assert!(error.starts_with("failed to read overlay "), "{error}");

    let path = temp_path("overlay-ok");
    std::fs::write(&path, "- insert:\n    - id: a\n      name: pkg\n").unwrap();
    let patches = load_overlay_patches(&path).unwrap();
    assert_eq!(patches.len(), 1);
    assert_eq!(
        patches[0].insert.as_ref().unwrap()[0].id.as_deref(),
        Some("a")
    );
    let _ = std::fs::remove_file(&path);
}

// --------------------------------------------------------- provenance ---

/// The upstream dump spec base: two rows, one with a config the surface
/// layer rewrites and one untouched.
fn dump_base() -> Vec<EntryOptions> {
    vec![
        entry("shared", "./noop.mjs").with_config(Node::from_iter([
            ("value".to_string(), "base".into()),
            ("key".to_string(), "${{ env.DSH_DUMP_SPEC }}".into()),
        ])),
        entry("untouched", "./noop.mjs"),
    ]
}

#[test]
fn provenance_tracks_origin_and_patching_layers() {
    let surface = patches(
        "\
- id: shared
  config:
    value: surface
    key: ${{ env.DSH_DUMP_SPEC }}
- insert:
    - id: surface-extra
      name: ./noop.mjs
",
    );
    let user = patches("- id: surface-extra\n  config:\n    value: user\n");
    let layers = [
        DumpLayer {
            label: "surface.yml",
            patches: &surface,
        },
        DumpLayer {
            label: "user.yml",
            patches: &user,
        },
    ];
    let (composed, provenance) = compose_with_provenance(&dump_base(), "base.yml", &layers, |_| {});
    assert_eq!(
        provenance,
        vec![
            Provenance {
                origin: "base.yml".into(),
                patched_by: vec!["surface.yml".into()]
            },
            Provenance {
                origin: "base.yml".into(),
                patched_by: vec![]
            },
            Provenance {
                origin: "surface.yml".into(),
                patched_by: vec!["user.yml".into()]
            },
        ],
    );
    assert_eq!(
        composed[0].config,
        Some(Node::from_iter([
            ("value".to_string(), "surface".into()),
            ("key".to_string(), "${{ env.DSH_DUMP_SPEC }}".into()),
        ]))
    );
    assert_eq!(
        composed[2].config,
        Some(Node::from_iter([("value".to_string(), "user".into())]))
    );
}

#[test]
fn warnings_are_attributed_to_their_layer_by_prefix_tail() {
    let overlay = patches(
        "\
- id: only-on-another-surface
  config:
    value: ignored
- id: shared
  config:
    value: patched
",
    );
    let layers = [DumpLayer {
        label: "overlay.yml",
        patches: &overlay,
    }];
    let (lines, sink) = warning_sink();
    let (composed, _) = compose_with_provenance(&dump_base(), "base.yml", &layers, sink);
    assert_eq!(
        lines.borrow().to_vec(),
        ["[overlay.yml] patch: entry \"only-on-another-surface\" not found"],
    );
    // Composition keeps going past the skipped patch.
    assert_eq!(config_of(&composed[0])["value"], "patched".into());
}

#[test]
fn provenance_single_flatten_matches_the_composition_boot_performs() {
    // Upstream config-dump parity case: layer a's plain config replacement
    // introduces an entry-shaped row the single-pass index never sees, so
    // layer b's patch on it warns (attributed to b) and the row stays as
    // layer a wrote it — b never appears as a patcher of the group row.
    let base = vec![entry("g", "group")];
    let layer_a = patches(
        "\
- id: g
  config:
    - id: child
      name: pkg
      config:
        v: 1
",
    );
    let layer_b = patches("- id: child\n  config:\n    v: 2\n");
    let layers = [
        DumpLayer {
            label: "a.yml",
            patches: &layer_a,
        },
        DumpLayer {
            label: "b.yml",
            patches: &layer_b,
        },
    ];
    let (lines, sink) = warning_sink();
    let (composed, provenance) = compose_with_provenance(&base, "base.yml", &layers, sink);
    assert_eq!(
        lines.borrow().to_vec(),
        ["[b.yml] patch: entry \"child\" not found"]
    );
    let introduced = composed[0]
        .config
        .as_ref()
        .and_then(Node::as_array)
        .unwrap();
    assert_eq!(
        introduced[0].as_object().unwrap()["config"]["v"],
        Node::Int(1)
    );
    assert_eq!(
        provenance,
        vec![Provenance {
            origin: "base.yml".into(),
            patched_by: vec!["a.yml".into()]
        }],
    );
}

// ----------------------------------------------------------- rendering ---

#[test]
fn dump_renders_grouped_sections_and_round_trips() {
    let surface = patches(
        "\
- id: shared
  config:
    value: surface
    key: ${{ env.DSH_DUMP_SPEC }}
- insert:
    - id: surface-extra
      name: ./noop.mjs
",
    );
    let user = patches("- id: surface-extra\n  config:\n    value: user\n");
    let layers = [
        DumpLayer {
            label: "surface.yml",
            patches: &surface,
        },
        DumpLayer {
            label: "user.yml",
            patches: &user,
        },
    ];
    let (composed, _) = compose_with_provenance(&dump_base(), "base.yml", &layers, |_| {});
    let dump = render_dump(
        &composed,
        &[
            Provenance {
                origin: "base.yml".into(),
                patched_by: vec!["surface.yml".into()],
            },
            Provenance {
                origin: "base.yml".into(),
                patched_by: vec![],
            },
            Provenance {
                origin: "surface.yml".into(),
                patched_by: vec!["user.yml".into()],
            },
        ],
    )
    .unwrap();

    // Comments do not break loadability: the dump parses as one document
    // equal to what a boot would mount.
    let parsed: Vec<EntryOptions> = serde_yaml_ng::from_str(&dump).unwrap();
    assert_eq!(parsed, composed);

    // Templates print verbatim, unevaluated.
    assert!(dump.contains("key: ${{ env.DSH_DUMP_SPEC }}"), "{dump}");
    // Source separators: origin file plus every layer that changed the row;
    // an inserted row carries the inserting layer as its origin.
    assert!(
        dump.contains("# == base.yml, patched by surface.yml"),
        "{dump}"
    );
    assert!(dump.contains("# == base.yml\n- id: untouched"), "{dump}");
    assert!(
        dump.contains("# == surface.yml, patched by user.yml\n- id: surface-extra"),
        "{dump}"
    );
    assert!(
        dump.find("# == base.yml, patched by surface.yml")
            < dump.find("# == base.yml\n- id: untouched"),
        "{dump}"
    );
}

#[test]
fn dump_groups_contiguous_rows_under_one_separator() {
    let dump = render_config_dump(&dump_base(), "base.yml", &[], |_| {}).unwrap();
    assert_eq!(dump.matches("# == base.yml").count(), 1);
    assert!(dump.contains("# == base.yml\n- id: shared"), "{dump}");
    // The whole document stays loadable.
    let parsed: Vec<EntryOptions> = serde_yaml_ng::from_str(&dump).unwrap();
    assert_eq!(parsed, dump_base());
}

#[test]
fn dump_via_convenience_matches_the_split_form() {
    let overlay = patches("- id: shared\n  disabled: true\n");
    let layers = [DumpLayer {
        label: "overlay.yml",
        patches: &overlay,
    }];
    let (composed, provenance) = compose_with_provenance(&dump_base(), "base.yml", &layers, |_| {});
    let split = render_dump(&composed, &provenance).unwrap();
    let direct = render_config_dump(&dump_base(), "base.yml", &layers, |_| {}).unwrap();
    assert_eq!(split, direct);
    assert!(
        split.contains("# == base.yml, patched by overlay.yml"),
        "{split}"
    );
}

#[test]
fn dump_renders_group_children_and_entry_field_order() {
    let base = vec![entry("srv", "group").with_group(vec![
        entry("adapter-http", "adapter-http").with_config(Node::from_iter([
            ("port".to_string(), Node::Int(8080)),
        ])),
    ])];
    let dump = render_config_dump(&base, "cordis.yml", &[], |_| {}).unwrap();
    let parsed: Vec<EntryOptions> = serde_yaml_ng::from_str(&dump).unwrap();
    assert_eq!(parsed, base);
    // Field order id/name first, config last — stable in dumps.
    let id_pos = dump.find("id: srv").expect("id present");
    let name_pos = dump.find("name: group").expect("name present");
    let config_pos = dump.rfind("config:").expect("config present");
    assert!(id_pos < name_pos && name_pos < config_pos, "{dump}");
}
