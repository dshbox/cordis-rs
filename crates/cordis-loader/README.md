# cordis-loader

**English** | [简体中文](README.zh-CN.md)

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

## Dynamic library plugins

With the `dynamic` feature, entries can also resolve to plugins compiled
as `cdylib` libraries — Rust has no stable ABI, so this is gated on a
strict build fingerprint (cordis-rs version, exact rustc including commit
hash, target triple, panic strategy): a library built by anything other
than the loading process' own toolchain is rejected instead of causing
undefined behavior.

On the plugin side, a `crate-type = ["cdylib"]` crate depending on
`cordis-loader` with the `dynamic` feature implements `Plugin` and ends
with the export macro (file name decides the plugin name):

```rust,ignore
// greeter-plugin/src/lib.rs — builds libgreeter.so / libgreeter.dylib /
// greeter.dll
use cordis_loader::dynamic::{BoxFuture, Config, Context, Plugin, PluginOutput, Result};

pub struct Greeter;

impl Plugin for Greeter {
    fn name(&self) -> &str { "greeter" }

    fn apply(&self, _ctx: Context, _config: Config) -> BoxFuture<Result<PluginOutput>> {
        Box::pin(async { Ok(PluginOutput::none()) })
    }
}

cordis_loader::dynamic::export_plugin!(Greeter);
```

On the loading side, attach directories to the registry; names missing
from the static registry resolve from `lib<name>.so` (`.dylib` on macOS,
`<name>.dll` on Windows) there:

```rust
# use cordis_loader::PluginRegistry;
let registry = PluginRegistry::new().with_dynamic_dirs(["/usr/lib/cordis-plugins"]);
# assert!(registry.names().any(|name| name == "group"));
```

Each resolve asks the library for a fresh plugin instance in a fresh
handle. Panics are contained on the plugin side — a `cdylib` links its
own std, so the export macro wraps every callback in a guard that turns
panics into errors and fallback values before they can cross the
boundary. Libraries are never unloaded within a process, and a replaced
library file requires a fresh process — which is exactly the HMR flow
`cordis-cli` drives with `cordis run --plugin-dir <dir>`: it watches the
directories and hot-restarts the worker (exit code 51) when a library
changes. See the `dynamic` module docs for the full safety model.

## What the state machine does

- **open** — read (or create) the entry file, build the tree, start every
  enabled entry; group children start beneath their group fiber's context,
  so disposing a group cascades. A corrupt or unreadable main file fails
  the open instead of silently starting an empty tree.
- **reload** — re-read the file and reconcile (serialized against
  `update_config` and `dispose`): created entries start, removed subtrees
  stop, moved entries restart under their new parent, entries whose plugin
  name / inject declaration / enabled flag changed stop and restart with
  their new options, and config-only changes patch in place — a patch the
  plugin rejects keeps the fiber on its old config and is retried by the
  next reload. Generated ids are persisted afterwards so the next reload
  matches them.
- **dispose** — stop every entry, stop watching files, and release the
  loader's root-level effects (the status listener and the `loader`
  service); a fresh `Loader::open` on the same root works afterwards.
- **self-kill** — a fiber that reaches `Disposed` outside loader operation
  was killed by its own plugin; the loader persists `disabled: true` for
  that entry shortly after (deferred off the dying fiber's transition
  lock). Removing an entry from the file just stops it.
- **inject** — an entry's `inject` list is merged with the plugin's own
  declaration (both gate startup), so services going away or coming back
  reconciles entries through the core machinery. The import graph must be
  a tree: cycles and duplicate mounts of one file are reported distinctly
  through `last_error()`.
- **update_config** — the runtime entry point for changing config: updates
  the fiber and persists to the file.

## Events and write coalescing

The loader emits `loader/entry-init`, `loader/before-patch`,
`loader/after-patch`, `loader/partial-dispose`, and
`loader/config-update` on the root context's event bus — every listener
gets the affected entry, `config-update` also the new config node. Write
debouncing coalesces rapid write-backs:

```rust
# use cordis_loader::{Loader, LoaderConfig};
# use std::time::Duration;
# let root = cordis::Context::new();
# let config = LoaderConfig::new("cordis.yml").with_write_debounce(Duration::from_millis(300));
let loader = Loader::open(&root, config)?;
// loader.update_config(...) calls now merge into one disk write
// after 300ms of quiet; loader.file().flush_deferred() waits it out.
# Ok::<(), cordis_loader::LoaderError>(())
```

## Feature flags

- **`watch`** — hot reload: wires `LoaderFile`'s debounced watcher to
  [`Loader::reload`]. Reload errors are recorded in `last_error()`.
- **`dynamic`** — resolve entries from dynamic-library plugins
  (`libloading`): fingerprint-checked loading through
  `PluginRegistry::with_dynamic_dirs`, the `export_plugin!` macro for
  plugin crates, and panic containment on the plugin side.

The [`cordis-cli`](https://crates.io/crates/cordis-cli) runner builds its
`cordis run` command on top of this crate.

[`Loader::reload`]: https://docs.rs/cordis-loader/latest/cordis_loader/struct.Loader.html#method.reload
