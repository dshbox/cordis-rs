# cordis-loader

Config-file driven plugin loader for the
[cordis-rs](https://crates.io/crates/cordis-rs) plugin framework.

This crate is the assembly half of porting upstream Cordis' loader: it
connects [cordis-include](https://crates.io/crates/cordis-include) entry
trees to cordis fibers and re-exports everything needed on top
(`cordis-include`, `cordis-group`), so applications depend on this crate
alone.

```text
┌─ cordis-loader   ← this crate: plugin registry + fiber state machine
├─ cordis-group    group plugin (nesting marker)
├─ cordis-include  entry trees + config files
└─ cordis-rs       core runtime (zero dependencies)
```

## Example

```rust
use cordis::{plugin_sync, Inject, PluginOutput};
use cordis_include::{Document, EntryOptions, Node};
use cordis_loader::{Loader, LoaderConfig, PluginRegistry};

# fn main() -> cordis_loader::Result<()> {
// Register plugins by name (upstream's dynamic import() replacement).
// The factory yields a fresh handle per entry.
let mut registry = PluginRegistry::new();
registry.register("greeter", || {
    plugin_sync::<Node, _>(
        "greeter",
        Inject::default(),
        |_ctx, config| {
            let port = config["port"].as_i64().unwrap_or(80);
            println!("greeter on port {port}");
            Ok(PluginOutput::none())
        },
    )
});

let path = std::env::temp_dir().join(format!("cordis-loader-readme-{}.yml", std::process::id()));
let initial = Document::with_entries(vec![
    EntryOptions::new("greeter")
        .with_id("greet")
        .with_config([("port".to_string(), Node::Int(8080))].into_iter().collect()),
]);

let root = cordis::Context::new();
let loader = Loader::open(
    &root,
    LoaderConfig::new(&path).with_registry(registry).with_initial(initial),
)?;

let entry = loader.tree().resolve("greet").unwrap();
entry.fiber().unwrap().try_wait()?;      // started from the file
loader.update_config("greet", [("port".to_string(), Node::Int(9090))].into_iter().collect())?;
entry.fiber().unwrap().try_wait()?;      // restarted with the new config

loader.dispose()?;
let _ = std::fs::remove_file(&path);
# Ok(())
# }
```

## Imports

An entry with `name: import` and `config: { url: "…" }` mounts another
config file as its subtree — same diff machinery, same id reuse:

```yaml
# main.yml
entries:
  - id: extra
    name: import
    config:
      url: extra.yml
```

```yaml
# extra.yml — entries become children of `extra`
entries:
  - id: adapter
    name: adapter-http
```

Reloads compose all involved files into one tree diff; write-back always
routes mounted entries back to the file they came from (generated ids
included). Import cycles are reported via `last_error()` and skipped.

## What the state machine does

- **open** — read (or create) the entry file, build the tree, start every
  enabled entry; group children start beneath their group fiber's context,
  so disposing a group cascades.
- **reload** — re-read the file under a suspend guard and reconcile:
  created entries start, removed subtrees stop, moved entries restart
  under their new parent, config-only changes patch in place. Generated
  ids are persisted afterwards so the next reload matches them.
- **self-kill** — a fiber that reaches `Disposed` outside loader operation
  was killed by its own plugin; the loader persists `disabled: true` for
  that entry. Removing an entry from the file just stops it.
- **inject** — an entry's `inject` list is merged into the plugin's own
  declaration, so services going away or coming back reconciles entries
  through the core machinery.
- **update_config** — the runtime entry point for changing config: updates
  the fiber and persists to the file.

## Feature flags

- **`watch`** — hot reload: wires `LoaderFile`'s debounced watcher to
  [`Loader::reload`]. Reload errors are recorded in `last_error()`.

The [`cordis-cli`](https://crates.io/crates/cordis-cli) runner builds its
`cordis run` command on top of this crate.

[`Loader::reload`]: https://docs.rs/cordis-loader/latest/cordis_loader/struct.Loader.html#method.reload
