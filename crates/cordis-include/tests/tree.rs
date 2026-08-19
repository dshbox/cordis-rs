//! Entry tree structure, id scheme, and full-diff reload behavior.

use cordis_include::{Entry, EntryOptions, EntryTree, IncludeError, Node};

fn node(pairs: &[(&str, Node)]) -> Node {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.clone()))
        .collect()
}

#[test]
fn create_assigns_ids_and_resolves_composite_paths() {
    let tree = EntryTree::new();
    let group = tree
        .create(
            EntryOptions::new("group")
                .with_id("outer")
                .with_config(node(&[("k", "v".into())])),
            None,
            None,
        )
        .unwrap();
    let child = tree
        .create(EntryOptions::new("plugin-a"), Some(&group), None)
        .unwrap();

    assert_eq!(child.id().len(), 6);
    assert!(child.id().bytes().all(|b| b.is_ascii_alphanumeric()));
    assert_eq!(child.path(), format!("outer:{}", child.id()));
    assert_eq!(group.path(), "outer");

    let resolved = tree.resolve(&child.path()).unwrap();
    assert!(Entry::ptr_eq(&resolved, &child));
    assert!(tree.resolve("nope").is_none());
}

#[test]
fn invalid_and_duplicate_ids_are_rejected() {
    let tree = EntryTree::new();
    tree.create(EntryOptions::new("a").with_id("x:y"), None, None)
        .unwrap_err();
    tree.create(EntryOptions::new("a").with_id(""), None, None)
        .unwrap_err();
    tree.create(EntryOptions::new("a").with_id("dup"), None, None)
        .unwrap();
    match tree.create(EntryOptions::new("b").with_id("dup"), None, None) {
        Err(IncludeError::DuplicateId { id }) => assert_eq!(id, "dup"),
        other => panic!("expected DuplicateId, got {other:?}"),
    }
    tree.create(EntryOptions::new(""), None, None).unwrap_err();
}

#[test]
fn update_reuses_entries_and_reports_the_diff() {
    let tree = EntryTree::new();
    let initial = vec![
        EntryOptions::new("keep").with_id("k"),
        EntryOptions::new("mover").with_id("d"),
    ];
    let diff = tree.reconcile(initial).unwrap();
    assert_eq!(diff.created.len(), 2);
    assert!(!diff.is_empty());

    let kept_before = tree.resolve("k").unwrap();
    let mover_before = tree.resolve("d").unwrap();
    let reload = vec![
        // same id, new config -> updated, same Entry object
        EntryOptions::new("keep")
            .with_id("k")
            .with_config(node(&[("port", 8080.into())])),
        // fresh id -> created
        EntryOptions::new("fresh"),
        // the top-level entry "d" reappears inside a new group -> moved
        EntryOptions::new("group")
            .with_id("g")
            .with_group(vec![EntryOptions::new("mover").with_id("d")]),
    ];
    let diff = tree.reconcile(reload).unwrap();
    assert!(diff.created.iter().any(|e| e.id() == "g"));
    assert!(diff.updated.iter().any(|e| e.id() == "k"));
    assert!(diff.moved.iter().any(|e| e.id() == "d"));
    assert!(diff.removed.is_empty());

    // "d" was reused (moved), so the live tree still holds the same object.
    let moved = tree.resolve("g:d").unwrap();
    assert!(Entry::ptr_eq(&moved, &mover_before));

    let kept_after = tree.resolve("k").unwrap();
    assert!(Entry::ptr_eq(&kept_before, &kept_after));
    assert_eq!(kept_after.config().unwrap()["port"], Node::Int(8080));

    // Generated id for "fresh" survives serialization.
    let serialized = tree.serialize();
    let fresh = serialized
        .iter()
        .find(|options| options.name == "fresh")
        .unwrap();
    assert_eq!(fresh.id.as_deref().map(str::len), Some(6));
}

/// Regression (review 2026-08-19, REV-01): a whole-tree update re-parents a
/// top-level entry into a nested group during `sync_children`, and the
/// root's final `set_children` pass used to clear that fresh parent link —
/// the moved entry lost `path()`, ancestor cascades, and (at the loader
/// layer) its group fiber context.
#[test]
fn update_keeps_the_parent_link_of_entries_moved_into_groups() {
    let tree = EntryTree::new();
    tree.reconcile(vec![
        EntryOptions::new("keep").with_id("k"),
        EntryOptions::new("mover").with_id("d"),
        EntryOptions::new("worker").with_id("w"),
    ])
    .unwrap();

    let reload = vec![
        EntryOptions::new("keep").with_id("k"),
        // "d" moves from the top level directly into g; "w" moves into
        // g's child, so its re-parenting happens two levels below the root.
        EntryOptions::new("group").with_id("g").with_group(vec![
            EntryOptions::new("mover")
                .with_id("d")
                .with_group(vec![EntryOptions::new("worker").with_id("w")]),
        ]),
    ];
    let diff = tree.reconcile(reload).unwrap();
    assert!(diff.moved.iter().any(|e| e.id() == "d"));
    assert!(diff.moved.iter().any(|e| e.id() == "w"));
    assert!(diff.removed.is_empty());

    let group = tree.resolve("g").unwrap();
    let moved = tree.resolve("g:d").unwrap();
    let nested = tree.resolve("g:d:w").unwrap();
    assert_eq!(moved.path(), "g:d");
    assert_eq!(nested.path(), "g:d:w");
    assert!(moved.parent().is_some_and(|p| Entry::ptr_eq(&p, &group)));
    assert!(nested.parent().is_some_and(|p| Entry::ptr_eq(&p, &moved)));
    assert!(moved.is_enabled() && nested.is_enabled());

    // Disabling the group must cascade through the preserved links.
    tree.reconcile(vec![
        EntryOptions::new("group")
            .with_id("g")
            .with_disabled(true)
            .with_group(vec![
                EntryOptions::new("mover")
                    .with_id("d")
                    .with_group(vec![EntryOptions::new("worker").with_id("w")]),
            ]),
    ])
    .unwrap();
    assert!(!moved.is_enabled(), "ancestor cascade reaches moved entry");
    assert!(
        !nested.is_enabled(),
        "ancestor cascade reaches nested entry"
    );

    // Genuine removal still detaches: the parent link of a dropped child
    // is cleared, not kept by mistake.
    tree.reconcile(vec![EntryOptions::new("group").with_id("g")])
        .unwrap();
    assert_eq!(moved.parent().map(|p| p.id().to_string()), None);
    assert_eq!(moved.path(), "d");
}

#[test]
fn update_rejects_duplicate_ids_atomically() {
    let tree = EntryTree::new();
    tree.reconcile(vec![EntryOptions::new("a").with_id("one")])
        .unwrap();
    let bad = vec![
        EntryOptions::new("a").with_id("same"),
        EntryOptions::new("b").with_id("same"),
    ];
    assert!(matches!(
        tree.reconcile(bad),
        Err(IncludeError::DuplicateId { .. })
    ));
    // The failed reload left the existing tree untouched.
    assert!(tree.resolve("one").is_some());
    assert_eq!(tree.entries().len(), 1);
}

#[test]
fn remove_detaches_but_keeps_the_subtree() {
    let tree = EntryTree::new();
    let group = tree
        .create(EntryOptions::new("group").with_id("g"), None, None)
        .unwrap();
    let child = tree
        .create(EntryOptions::new("child"), Some(&group), None)
        .unwrap();

    let detached = tree.remove("g").unwrap();
    assert!(Entry::ptr_eq(&detached, &group));
    assert!(tree.resolve("g").is_none());
    assert!(group.parent().is_none());
    // The subtree survives for teardown.
    assert!(Entry::ptr_eq(&detached.children()[0], &child));
    assert!(tree.remove("missing").is_err());
}

#[test]
fn reconcile_entry_moves_and_rejects_cycles() {
    let tree = EntryTree::new();
    let outer = tree
        .create(EntryOptions::new("group").with_id("outer"), None, None)
        .unwrap();
    let inner = tree
        .create(
            EntryOptions::new("group").with_id("inner"),
            Some(&outer),
            None,
        )
        .unwrap();

    // Move `outer` beneath its own child: must fail.
    assert!(matches!(
        tree.reconcile_entry("outer", EntryOptions::new("group"), Some(&inner), None),
        Err(IncludeError::Cycle)
    ));

    // Move `inner` (nested under `outer`) to the top level and rename it.
    let moved = tree
        .reconcile_entry(
            "outer:inner",
            EntryOptions::new("renamed"),
            Some(tree.root()),
            Some(0),
        )
        .unwrap();
    assert_eq!(moved.name(), "renamed");
    assert_eq!(tree.top_level()[0].id(), "inner");
    assert!(outer.children().is_empty());
}

#[test]
fn disabled_cascades_through_ancestors() {
    let tree = EntryTree::new();
    tree.reconcile(vec![
        EntryOptions::new("group")
            .with_id("g")
            .with_group(vec![EntryOptions::new("child").with_id("c")]),
    ])
    .unwrap();
    let group = tree.resolve("g").unwrap();
    let child = tree.resolve("g:c").unwrap();

    assert!(child.is_enabled());
    tree.reconcile(vec![
        EntryOptions::new("group")
            .with_id("g")
            .with_disabled(true)
            .with_group(vec![EntryOptions::new("child").with_id("c")]),
    ])
    .unwrap();
    assert!(!group.is_enabled());
    assert!(group.is_disabled());
    assert!(!child.is_disabled(), "own flag untouched");
    assert!(!child.is_enabled(), "ancestor cascade applies");
}

#[test]
fn serialize_round_trips_nested_groups() {
    let tree = EntryTree::new();
    tree.reconcile(vec![EntryOptions::new("group").with_id("g").with_group(
        vec![
        EntryOptions::new("child").with_id("c").with_config(node(&[("n", 1.into())])),
    ],
    )])
    .unwrap();

    let options = tree.serialize();
    assert_eq!(options.len(), 1);
    assert_eq!(options[0].group.len(), 1);
    assert_eq!(
        options[0].group[0].config.as_ref().unwrap()["n"],
        Node::Int(1)
    );

    // Reload from the serialized form: identity is preserved by id.
    let before = tree.resolve("g:c").unwrap();
    tree.reconcile(options).unwrap();
    let after = tree.resolve("g:c").unwrap();
    assert!(Entry::ptr_eq(&before, &after));
}

#[test]
fn suspend_guard_and_fiber_slot() {
    let tree = EntryTree::new();
    let entry = tree.create(EntryOptions::new("a"), None, None).unwrap();
    assert!(!entry.is_suspended());
    let guard = entry.suspend();
    assert!(entry.is_suspended());
    drop(guard);
    assert!(!entry.is_suspended());

    assert!(entry.fiber().is_none());
    let root = cordis::Context::new();
    let plugin =
        cordis::plugin_sync::<(), _>("test", cordis::Inject::new(["service"]), |_ctx, _config| {
            Ok(cordis::PluginOutput::default())
        });
    let fiber = root.plugin(plugin, ());
    entry.set_fiber(Some(fiber.clone()));
    assert!(entry.fiber().is_some());
}

#[test]
fn foreign_entries_are_rejected_as_parents() {
    let theirs = EntryTree::new();
    let outsider = theirs
        .create(EntryOptions::new("x").with_id("x"), None, None)
        .unwrap();
    let ours = EntryTree::new();
    assert!(matches!(
        ours.create(EntryOptions::new("a"), Some(&outsider), None),
        Err(IncludeError::NotInTree)
    ));
}
