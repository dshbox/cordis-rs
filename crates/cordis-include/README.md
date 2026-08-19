# cordis-include

**English** | [简体中文](README.zh-CN.md)

Config entry trees and YAML/JSON loader files for the
[cordis-rs](https://crates.io/crates/cordis-rs) plugin framework.

This crate is the data half of porting upstream Cordis' loader: it maps
between config files on disk and an in-memory tree of entries, preserving
object key order for diff-friendly files, expanding `${{ env.NAME }}`
templates, evaluating the `!!js` expression subset at hand-off, carrying
the patch algebra behind bundle/profile composition (apply, layer
composition, provenance dumps), and providing the suspend guards that
break the write → watch → write feedback loop.

```text
┌─ cordis-loader   assembly: plugin registry + fiber state machine
├─ cordis-group    group plugin (nesting marker)
├─ cordis-include  ← this crate: entry trees + config files
└─ cordis-rs       core runtime (zero dependencies)
```

## Example

```rust
use cordis_include::{Entry, EntryOptions, EntryTree, Node};
# fn main() -> cordis_include::Result<()> {
let tree = EntryTree::new();

// Load a set of entries (from a file, or built by hand).
let diff = tree.reconcile(vec![
    EntryOptions::new("group").with_id("srv").with_group(vec![
        EntryOptions::new("adapter-http").with_config(
            [("port".to_string(), Node::Int(8080))].into_iter().collect(),
        ),
    ]),
])?;
assert_eq!(diff.created.len(), 2);

// A full reconcile matches entries by id across groups: existing entry
// objects are reused (same pointer), so callers can keep their handles.
let kept = tree.resolve("srv").unwrap();
tree.reconcile(vec![EntryOptions::new("group").with_id("srv")])?;
assert!(Entry::ptr_eq(&kept, &tree.resolve("srv").unwrap()));
# Ok(())
# }
```

Files round-trip through [`LoaderFile`] with atomic, writer-serialized
`.tmp` + rename writes (concurrent writers cannot interleave),
readonly detection, unknown top-level keys preserved, and coalesced
deferred writes (`write_deferred`) for bursty callers. YAML parses
through the crate's own dialect: it matches the previous serde-based
reader (verified by A/B tests) while keeping `!!js` scalars as
expression nodes ([`Node`]) that round-trip verbatim, unevaluated:

```yaml
entries:
  - id: srv
    name: group
  - id: gated
    name: adapter-http
    disabled: !!js process.platform === 'win32'
    config:
      port: 8080
      host: ${{ env.HOST }}
      mode: !!js process.env.DSH_MODE || 'default'
```

At hand-off ([`Entry::resolved_config`]) every `!!js` expression evaluates
through the [`expr`] subset — the `process.*` references the shipped
bundles use; expressions touching injected context (`ctx.*`,
`dshHomePath(…)`) fail with a clear subset error. The `disabled` field
takes the same `!!js` form: the raw text round-trips through the file
and [`Entry::resolved_disabled`] evaluates it at activation.

## Patch lists

Entry lists compose from *patch* files — bare top-level YAML arrays of
[`PatchOptions`] rows (`id`-targeted overrides and `insert` lists), the
bundle/profile assembly model. [`apply_entry_patches`] is the one
application routine every consumer shares; [`compose_layers`] flattens all
layers into a single call (the same single call a boot performs);
[`render_config_dump`] prints the composition grouped by source under
`# ==` provenance comments:

```rust
use cordis_include::{compose_layers, EntryOptions, Node, PatchOptions};
# fn main() {
let bundle = vec![PatchOptions {
    insert: Some(vec![EntryOptions::new("adapter-http")
        .with_id("http")
        .with_config(Node::from_iter([("port".to_string(), 8080.into())]))]),
    ..Default::default()
}];
let user = vec![PatchOptions {
    id: Some("http".into()),
    disabled: Some(true),
    ..Default::default()
}];
let entries = compose_layers(&[bundle, user], |_| {});
assert_eq!(entries.len(), 1);
assert!(entries[0].disabled.is_disabled());
# }
```

## Feature flags

- **`watch`** — debounced file watching through [`notify`](https://crates.io/crates/notify).
  Events observed while the file is suspended — by a caller-held suspend
  guard, e.g. around the caller's own writes — do not fire the callback.

## Scope

This crate deliberately knows nothing about *where plugins come from* and
never starts or stops fibers: [`cordis-loader`](https://crates.io/crates/cordis-loader)
implements the [`PluginResolver`] contract defined here and drives the
lifecycle.

[`LoaderFile`]: https://docs.rs/cordis-include/latest/cordis_include/struct.LoaderFile.html
[`PluginResolver`]: https://docs.rs/cordis-include/latest/cordis_include/trait.PluginResolver.html
[`PatchOptions`]: https://docs.rs/cordis-include/latest/cordis_include/struct.PatchOptions.html
[`apply_entry_patches`]: https://docs.rs/cordis-include/latest/cordis_include/fn.apply_entry_patches.html
[`compose_layers`]: https://docs.rs/cordis-include/latest/cordis_include/fn.compose_layers.html
[`render_config_dump`]: https://docs.rs/cordis-include/latest/cordis_include/fn.render_config_dump.html
[`Entry::resolved_config`]: https://docs.rs/cordis-include/latest/cordis_include/struct.Entry.html#method.resolved_config
[`Entry::resolved_disabled`]: https://docs.rs/cordis-include/latest/cordis_include/struct.Entry.html#method.resolved_disabled
[`expr`]: https://docs.rs/cordis-include/latest/cordis_include/expr/index.html

[`Node`]: https://docs.rs/cordis-include/latest/cordis_include/enum.Node.html
