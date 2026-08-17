# cordis-include

Config entry trees and YAML/JSON loader files for the
[cordis-rs](https://crates.io/crates/cordis-rs) plugin framework.

This crate is the data half of porting upstream Cordis' loader: it maps
between config files on disk and an in-memory tree of entries, preserving
object key order for diff-friendly files, expanding `${{ env.NAME }}`
templates, and providing the suspend guards that break the
write → watch → write feedback loop.

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
let diff = tree.update(vec![
    EntryOptions::new("group").with_id("srv").with_group(vec![
        EntryOptions::new("adapter-http").with_config(
            [("port".to_string(), Node::Int(8080))].into_iter().collect(),
        ),
    ]),
])?;
assert_eq!(diff.created.len(), 2);

// A full reload matches entries by id across groups: existing entry
// objects are reused (same pointer), so callers can keep their handles.
let kept = tree.resolve("srv").unwrap();
tree.update(vec![EntryOptions::new("group").with_id("srv")])?;
assert!(Entry::ptr_eq(&kept, &tree.resolve("srv").unwrap()));
# Ok(())
# }
```

Files round-trip through [`LoaderFile`] with atomic `.tmp` + rename writes,
readonly detection, unknown top-level keys preserved, and coalesced
deferred writes (`write_deferred`) for bursty callers:

```yaml
entries:
  - id: srv
    name: group
    group:
      - name: adapter-http
        config:
          port: 8080
          host: ${{ env.HOST }}
```

## Feature flags

- **`watch`** — debounced file watching through [`notify`](https://crates.io/crates/notify).
  Events observed while the file is suspended (our own writes, or reloads in
  progress) do not fire the callback.

## Scope

This crate deliberately knows nothing about *where plugins come from* and
never starts or stops fibers: [`cordis-loader`](https://crates.io/crates/cordis-loader)
implements the [`PluginResolver`] contract defined here and drives the
lifecycle.

[`LoaderFile`]: https://docs.rs/cordis-include/latest/cordis_include/struct.LoaderFile.html
[`PluginResolver`]: https://docs.rs/cordis-include/latest/cordis_include/trait.PluginResolver.html
