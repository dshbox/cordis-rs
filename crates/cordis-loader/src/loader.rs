//! The loader: entry tree ⇄ fiber lifecycle, file reloads, write-back.

use crate::error::{LoaderError, Result};
use crate::lock;
use crate::registry::{PluginRegistry, WithInject};
use cordis::{Config, Context, EffectHandle, EventOptions, Fiber, FiberState, PluginHandle};
use cordis_include::{Entry, EntryOptions, EntryTree, LoaderFile, Node, PluginResolver, TreeDiff};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};

/// Where the loader reads and writes its entry file.
#[derive(Clone, Default)]
pub struct LoaderConfig {
    /// Path to the entry config file (`.yml`/`.yaml`/`.json`).
    pub filename: PathBuf,
    /// Document written on first run when the file does not exist yet.
    pub initial: Option<cordis_include::Document>,
    /// Plugin registry used to resolve entry names; defaults to a fresh
    /// [`PluginRegistry`] with only the `group` builtin.
    pub registry: Option<PluginRegistry>,
}

impl LoaderConfig {
    /// Configure a loader around `filename`.
    pub fn new(filename: impl Into<PathBuf>) -> Self {
        Self {
            filename: filename.into(),
            initial: None,
            registry: None,
        }
    }

    /// Provide the document written when the file is missing.
    pub fn with_initial(mut self, initial: cordis_include::Document) -> Self {
        self.initial = Some(initial);
        self
    }

    /// Provide the plugin registry entries resolve against.
    pub fn with_registry(mut self, registry: PluginRegistry) -> Self {
        self.registry = Some(registry);
        self
    }
}

/// Bookkeeping guarded by the loader's state lock.
struct LoaderState {
    /// fiber uid -> entry, for status-event routing and lookups.
    entries: HashMap<u64, Entry>,
    /// Non-zero while the loader itself drives fibers; self-kill detection
    /// ignores fibers disposed in that window.
    operating: u16,
    /// Last background error (reload callback, self-kill persistence).
    last_error: Option<String>,
    /// Keeps the internal listeners and the `loader` service registered.
    _keep_alive: Vec<EffectHandle>,
}

/// Cheap cloneable loader handle.
#[derive(Clone)]
pub struct Loader {
    pub(crate) inner: Arc<LoaderInner>,
}

pub(crate) struct LoaderInner {
    root: Context,
    file: LoaderFile,
    tree: EntryTree,
    registry: Mutex<PluginRegistry>,
    state: Mutex<LoaderState>,
}

/// Weak service handle injected as `loader`, avoiding a reference cycle
/// between the root context and the loader.
///
/// Recover the loader with [`LoaderHandle::upgrade`].
pub struct LoaderHandle {
    inner: Weak<LoaderInner>,
}

impl LoaderHandle {
    /// Upgrade to a strong loader reference, if still alive.
    pub fn upgrade(&self) -> Option<Loader> {
        self.inner.upgrade().map(|inner| Loader { inner })
    }
}

impl Loader {
    /// Open (creating if needed) the entry file, load the tree, and start
    /// every enabled entry.
    ///
    /// Entries that fail to resolve or start do not abort the open; the
    /// error is recorded and retrievable via [`Loader::last_error`], and the
    /// offending entry simply has no (or a failed) fiber.
    pub fn open(root: &Context, config: LoaderConfig) -> Result<Loader> {
        let file = LoaderFile::open(&config.filename)?;
        if !file.path().exists() {
            if let Some(initial) = &config.initial {
                file.write(initial)?;
            }
        }
        let tree = EntryTree::new();
        tree.update(file.read()?.entries)?;
        let inner = Arc::new(LoaderInner {
            root: root.clone(),
            file,
            tree,
            registry: Mutex::new(config.registry.unwrap_or_default()),
            state: Mutex::new(LoaderState {
                entries: HashMap::new(),
                operating: 0,
                last_error: None,
                _keep_alive: Vec::new(),
            }),
        });

        // The status listener routes plugin-initiated disposals (self-kill)
        // back into the config file as `disabled: true`.
        let weak = Arc::downgrade(&inner);
        let status = root.events().on(
            "internal/status",
            move |event| {
                if let Some(inner) = weak.upgrade() {
                    handle_status(&inner, &event)?;
                }
                Ok(None)
            },
            EventOptions {
                global: true,
                ..EventOptions::default()
            },
        )?;
        let service = root.provide_arc(
            "loader",
            Arc::new(LoaderHandle {
                inner: Arc::downgrade(&inner),
            }),
        )?;
        lock(&inner.state)._keep_alive = vec![status, service];

        let loader = Loader { inner };
        loader.start_all();
        Ok(loader)
    }

    /// The root context the loader operates on.
    pub fn context(&self) -> &Context {
        &self.inner.root
    }

    /// The entry tree.
    pub fn tree(&self) -> &EntryTree {
        &self.inner.tree
    }

    /// The entry config file.
    pub fn file(&self) -> &LoaderFile {
        &self.inner.file
    }

    /// The plugin registry (a clone of the current state); populate it via
    /// [`LoaderConfig::with_registry`] before open, or
    /// [`Loader::register_plugin`] later.
    pub fn registry(&self) -> PluginRegistry {
        lock(&self.inner.registry).clone()
    }

    /// Register one plugin instance by its own name; picked up by the next
    /// reload (or immediately for not-yet-started entries).
    pub fn register_plugin<P: cordis::Plugin>(&self, plugin: P) {
        lock(&self.inner.registry).register_plugin(plugin);
    }

    /// Register a handle factory under a name.
    pub fn register<F>(&self, name: impl Into<String>, factory: F)
    where
        F: Fn() -> PluginHandle + Send + Sync + 'static,
    {
        lock(&self.inner.registry).register(name, factory);
    }

    /// The last background error recorded by the loader, if any.
    pub fn last_error(&self) -> Option<String> {
        lock(&self.inner.state).last_error.clone()
    }

    /// The entry whose fiber is `fiber`, if the loader started it.
    pub fn locate(&self, fiber: &Fiber) -> Option<Entry> {
        let state = lock(&self.inner.state);
        if let Some(uid) = fiber.uid() {
            return state.entries.get(&uid).cloned();
        }
        state
            .entries
            .values()
            .find(|entry| entry.fiber().is_some_and(|started| started.ptr_eq(fiber)))
            .cloned()
    }

    /// Start every enabled, unstarted entry, parents before children.
    fn start_all(&self) {
        for entry in self.inner.tree.entries() {
            if let Err(error) = start_entry(&self.inner, &entry) {
                self.record_error(error);
            }
        }
    }

    /// Re-read the entry file and apply the difference to the fibers.
    ///
    /// Created entries start (parents first), removed subtrees stop, moved
    /// entries restart under their new parent, updated entries are patched
    /// in place when only config changed and restarted otherwise. While the
    /// reload runs, the file is suspended, so the patches it causes are not
    /// written back; generated ids are persisted afterwards.
    pub fn reload(&self) -> Result<TreeDiff> {
        let inner = &self.inner;
        let _suspend = inner.file.suspend();
        let document = inner.file.read()?;
        let diff = inner.tree.update(document.entries)?;

        for entry in &diff.removed {
            if let Err(error) = stop_entry(inner, entry) {
                self.record_error(error);
            }
        }
        for entry in &diff.moved {
            if let Err(error) = stop_entry(inner, entry) {
                self.record_error(error);
            }
        }
        for entry in &diff.updated {
            if let Err(error) = patch_entry(inner, entry) {
                self.record_error(error);
            }
        }
        for entry in &diff.created {
            if let Err(error) = start_entry(inner, entry) {
                self.record_error(error);
            }
        }
        drop(_suspend);

        // Entries created without explicit ids had one generated; persist
        // it so the next reload can match them.
        if !diff.created.is_empty() {
            write_back(inner)?;
        }
        Ok(diff)
    }

    /// Change one entry's config at runtime: the fiber is updated (and
    /// restarted when active) and the new config is persisted to the file.
    pub fn update_config(&self, id: &str, config: Node) -> Result<()> {
        let inner = &self.inner;
        let entry = inner.tree.resolve(id).ok_or_else(|| {
            LoaderError::Include(cordis_include::IncludeError::EntryNotFound { id: id.to_owned() })
        })?;
        if let Some(fiber) = entry.fiber() {
            fiber.update_value(Config::new(config.clone()))?;
        }
        let mut options = entry_options_with_children(&entry);
        options.config = Some(config);
        inner
            .tree
            .update_entry(&entry.path(), options, None, None)?;
        write_back(inner)
    }

    /// Stop every entry and clear the fiber map. The root context itself
    /// stays usable.
    pub fn dispose(&self) -> Result<()> {
        let inner = &self.inner;
        for entry in inner.tree.top_level() {
            if let Err(error) = stop_entry(inner, &entry) {
                self.record_error(error);
            }
        }
        Ok(())
    }

    /// Watch the entry file for external changes and reload on them
    /// (`watch` feature). Reload errors are recorded in
    /// [`Loader::last_error`].
    #[cfg(feature = "watch")]
    pub fn watch(&self) -> Result<cordis_include::FileWatcher> {
        let loader = self.clone();
        self.inner
            .file
            .watch(move || {
                if let Err(error) = loader.reload() {
                    loader.record_error(error);
                }
            })
            .map_err(LoaderError::Include)
    }

    fn record_error(&self, error: LoaderError) {
        lock(&self.inner.state).last_error = Some(error.to_string());
    }
}

/// Increment `operating` for the lifetime of the guard, so disposals driven
/// by the loader itself are not mistaken for self-kill.
struct OperatingGuard<'a> {
    state: &'a Mutex<LoaderState>,
}

impl<'a> OperatingGuard<'a> {
    fn new(state: &'a Mutex<LoaderState>) -> Self {
        lock(state).operating += 1;
        Self { state }
    }
}

impl Drop for OperatingGuard<'_> {
    fn drop(&mut self) {
        let mut state = lock(self.state);
        state.operating = state.operating.saturating_sub(1);
    }
}

/// Start one entry's fiber beneath its parent group's context.
fn start_entry(inner: &LoaderInner, entry: &Entry) -> Result<()> {
    if !entry.enabled() || entry.fiber().is_some() {
        return Ok(());
    }
    let name = entry.name();
    let handle: PluginHandle = lock(&inner.registry)
        .resolve(&name)
        .map_err(LoaderError::Cordis)?;
    let inject = entry.options().inject;
    let handle = WithInject::wrap(handle, inject);
    let config = entry.resolved_config()?.unwrap_or(Node::Null);
    let parent_ctx = entry
        .parent()
        .and_then(|parent| parent.fiber())
        .and_then(|fiber| fiber.context())
        .unwrap_or_else(|| inner.root.clone());
    let fiber = parent_ctx.plugin(handle, config);
    entry.set_fiber(Some(fiber.clone()));
    if let Some(uid) = fiber.uid() {
        lock(&inner.state).entries.insert(uid, entry.clone());
    }
    Ok(())
}

/// Stop one entry's fiber (children first for bookkeeping; disposal of a
/// group cascades regardless).
fn stop_entry(inner: &LoaderInner, entry: &Entry) -> Result<()> {
    for child in entry.children() {
        stop_entry(inner, &child)?;
    }
    let Some(fiber) = entry.fiber() else {
        return Ok(());
    };
    entry.set_fiber(None);
    if let Some(uid) = fiber.uid() {
        lock(&inner.state).entries.remove(&uid);
    }
    let _guard = OperatingGuard::new(&inner.state);
    fiber.dispose().map_err(LoaderError::Cordis)
}

/// Apply an options change to a live entry: restart when the plugin identity
/// changed (name, inject) or the enabled flag flipped; patch the config in
/// place otherwise.
fn patch_entry(inner: &LoaderInner, entry: &Entry) -> Result<()> {
    if !entry.enabled() {
        return stop_entry(inner, entry);
    }
    let Some(fiber) = entry.fiber() else {
        return start_entry(inner, entry);
    };
    let options = entry.options();
    let inject_changed =
        fiber.inject().names().collect::<Vec<_>>() != options.inject.iter().collect::<Vec<_>>();
    if fiber.name() != options.name || inject_changed {
        stop_entry(inner, entry)?;
        return start_entry(inner, entry);
    }
    let new_config = entry.resolved_config()?.unwrap_or(Node::Null);
    let current = fiber
        .config()
        .downcast::<Node>()
        .ok()
        .map(|node| (*node).clone());
    if current.as_ref() != Some(&new_config) {
        fiber.update_value(Config::new(new_config))?;
    }
    Ok(())
}

/// Serialize an entry together with its live subtree (used by update paths
/// that must not disturb children).
fn entry_options_with_children(entry: &Entry) -> EntryOptions {
    let mut options = entry.options();
    options.group = entry
        .children()
        .iter()
        .map(entry_options_with_children)
        .collect();
    options
}

/// Persist the current tree, preserving unknown top-level file keys.
fn write_back(inner: &LoaderInner) -> Result<()> {
    let mut document = inner.file.read()?;
    document.entries = inner.tree.serialize();
    inner.file.write(&document)?;
    Ok(())
}

/// Route `internal/status` disposals: a fiber that reached `Disposed`
/// outside loader operation was killed by its own plugin, so record
/// `disabled: true` in the tree and persist it.
fn handle_status(inner: &LoaderInner, event: &cordis::Event) -> cordis::EventResult {
    let Some(fiber) = event.arg::<Fiber>(0).ok().flatten() else {
        return Ok(None);
    };
    if fiber.state() != FiberState::Disposed {
        return Ok(None);
    }
    if lock(&inner.state).operating > 0 {
        return Ok(None);
    }
    let Some(entry) = lock(&inner.state)
        .entries
        .values()
        .find(|entry| entry.fiber().is_some_and(|started| started.ptr_eq(&fiber)))
        .cloned()
    else {
        return Ok(None);
    };
    if let Err(error) = persist_self_dispose(inner, &entry) {
        lock(&inner.state).last_error = Some(error.to_string());
    }
    Ok(None)
}

/// A plugin disposed itself: unmap the entry and persist `disabled: true`.
fn persist_self_dispose(inner: &LoaderInner, entry: &Entry) -> Result<()> {
    {
        let mut state = lock(&inner.state);
        let key = state
            .entries
            .iter()
            .find(|(_, mapped)| Entry::ptr_eq(mapped, entry))
            .map(|(uid, _)| *uid);
        if let Some(uid) = key {
            state.entries.remove(&uid);
        }
    }
    entry.set_fiber(None);
    let mut options = entry_options_with_children(entry);
    options.disabled = true;
    inner
        .tree
        .update_entry(&entry.path(), options, None, None)?;
    write_back(inner)
}
