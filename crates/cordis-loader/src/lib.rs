//! Config-file driven plugin loader for
//! [cordis-rs](https://crates.io/crates/cordis-rs).
//!
//! This crate is the assembly half of porting upstream Cordis' loader: it
//! connects `cordis-include`'s entry trees to cordis fibers. Everything a
//! config-driven cordis application needs is re-exported here — depend on
//! `cordis-loader` alone.
//!
//! # How it works
//!
//! - **Static registry** ([`PluginRegistry`]) replaces upstream's dynamic
//!   `import(name)`: register plugins at startup, entries resolve by name.
//!   The `group` builtin is pre-registered. With the `dynamic` feature,
//!   names can also resolve to plugins compiled as dynamic libraries (see
//!   the [`dynamic`] module).
//! - **Startup**: [`Loader::open`] reads the entry file (writing
//!   `initial` when missing), builds the [`EntryTree`], and starts every
//!   enabled entry — group entries as `cordis_group::Group` fibers, children
//!   beneath their parent group's context, so disposing a group cascades.
//! - **Config**: entries carry a `cordis_include::Node` config (never `()`);
//!   loader plugins read it via `config.downcast::<Node>()`. `${{ env.X }}`
//!   templates expand at hand-off time; the file keeps the raw text.
//! - **Reload** ([`Loader::reload`], wired to the `watch` feature):
//!   re-read the file, diff the tree, and reconcile fibers — created entries
//!   start, removed subtrees stop, moved entries restart under their new
//!   parent, entries whose plugin name / inject declaration / enabled flag
//!   changed stop and restart with their new options, and config-only
//!   changes patch in place via `Fiber::update_value`. A corrupt or
//!   unreadable main file fails the operation instead of silently booting
//!   an empty tree (import files keep a tolerant record-and-skip path).
//! - **Inject**: an entry's `inject` list is merged into the plugin's own
//!   declaration, so the core fiber machinery reconciles entries when
//!   services come and go — "hot-swapped service restarts its dependents"
//!   for free.
//! - **Self-kill vs. removal**: a fiber that reaches `Disposed` outside
//!   loader operation was killed by its own plugin; the loader persists
//!   `disabled: true` for that entry shortly after, deferred off the dying
//!   fiber's transition lock. Removing an entry from the file just stops
//!   it.
//! - **Write-back**: [`Loader::update_config`] is the runtime entry point —
//!   it updates the fiber *and* persists the config. Reloads apply their
//!   patches without writing them back; only newly generated ids are
//!   persisted. `reload`, `update_config`, and `dispose` serialize through
//!   one operation lock (reentrant from event listeners), so a
//!   watch-thread reload cannot interleave with a plugin-thread
//!   `update_config`.
//!
//! # Example
//!
//! ```
//! use cordis_loader::{Loader, LoaderConfig, PluginRegistry};
//!
//! let root = cordis::Context::new();
//! let mut registry = PluginRegistry::new();
//! // registry.register_plugin(my_plugin);  // your plugins, by name
//! let config = LoaderConfig::new("cordis.yml").with_registry(registry);
//! let loader = Loader::open(&root, config)?;
//! # assert!(loader.tree().entries().is_empty());
//! # Ok::<(), cordis_loader::LoaderError>(())
//! ```
//!
//! Register the plugins first (the `group` builtin is pre-registered), then
//! open; plugins registered later via [`Loader::register_plugin`] are picked
//! up by the next [`Loader::reload`].
//!
//! # Imports
//!
//! An entry with `name: import` and `config: { url: "…" }` mounts another
//! config file as its subtree. Reloads compose every involved file into
//! one tree (so diffs and id reuse work across files), while write-back
//! decomposes: mounted children are persisted to the file they came from,
//! never to the importing file. Import cycles are reported through
//! [`Loader::last_error`] instead of recursing. With the `watch` feature,
//! import files are watched like the main file.
//!
//! # Events and write coalescing
//!
//! Lifecycle transitions are observable through the [`events`] module's
//! event names on the root context's bus; listener failures are recorded,
//! never propagated. Write-backs can be debounced via
//! [`LoaderConfig::with_write_debounce`] or
//! [`Loader::set_write_debounce`]: rapid successive writes coalesce into
//! one physical write after the quiet window.
//!
//! # Dynamic library plugins
//!
//! With the `dynamic` feature, plugins can be compiled as `cdylib`
//! libraries and resolved from a directory instead of being registered
//! statically:
//!
//! ```rust,ignore
//! let registry = PluginRegistry::new().with_dynamic_dirs(["./plugins"]);
//! ```
//!
//! A plugin library exports its implementation through
//! [`dynamic::export_plugin!`] and must be built by the exact same
//! toolchain, target, panic strategy, and cordis-rs version as the loading
//! process — the loader verifies a build fingerprint before accepting the
//! library. Libraries are never unloaded within a process; reloading a
//! changed library is the worker-restart HMR flow implemented by
//! `cordis-cli`.
//!
//! # Not in scope yet
//!
//! Isolate/service migration is future work.

// `deny` instead of `forbid` because the `dynamic` feature wraps
// libloading's unsafe primitives; every unsafe operation lives in that one
// module, item-scoped behind `#[allow(unsafe_code)]` with SAFETY notes
// (the same pattern cordis-cli uses for dotenv).
#![deny(unsafe_code)]
#![warn(missing_docs)]

#[cfg(feature = "dynamic")]
pub mod dynamic;
pub mod error;
pub mod events;
pub mod loader;
pub mod registry;

pub use cordis_group::Group;
pub use cordis_include::{Document, Entry, EntryOptions, EntryTree, LoaderFile, Node};
pub use error::{LoaderError, Result};
pub use loader::{Loader, LoaderConfig, LoaderHandle};
pub use registry::PluginRegistry;

/// Lock a mutex tolerantly, treating a poisoned lock as unlocked.
pub(crate) fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}
