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
//!   The `group` builtin is pre-registered.
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
//!   parent, config-only changes patch in place via `Fiber::update_value`.
//! - **Inject**: an entry's `inject` list is merged into the plugin's own
//!   declaration, so the core fiber machinery reconciles entries when
//!   services come and go — "hot-swapped service restarts its dependents"
//!   for free.
//! - **Self-kill vs. removal**: a fiber that reaches `Disposed` outside
//!   loader operation was killed by its own plugin; the loader persists
//!   `disabled: true` for that entry. Removing an entry from the file just
//!   stops it.
//! - **Write-back**: [`Loader::update_config`] is the runtime entry point —
//!   it updates the fiber *and* persists the config. Reloads never echo
//!   back (file-level suspend).
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
//! # Not in scope yet
//!
//! Isolate/service migration and dynamic library plugins are future work.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

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
