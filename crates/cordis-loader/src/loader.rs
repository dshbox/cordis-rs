//! The loader: entry tree ⇄ fiber lifecycle, file reloads, write-back.

use crate::error::{LoaderError, Result};
use crate::lock;
use crate::registry::{PluginRegistry, WithInject};
use cordis::{
    Config, Context, CordisError, EffectHandle, ErrorCode, EventOptions, Fiber, FiberState,
    PluginHandle, Value,
};
use cordis_include::{Entry, EntryOptions, EntryTree, LoaderFile, Node, PluginResolver, TreeDiff};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::ThreadId;
use std::time::Duration;

/// Where the loader reads and writes its entry file.
#[derive(Clone, Default)]
pub struct LoaderConfig {
    /// Path to the entry config file (`.yml`/`.yaml`/`.json`).
    pub filename: PathBuf,
    /// Document written on first run when the file does not exist yet.
    pub initial: Option<cordis_include::Document>,
    /// Document composed from instead of reading the entry file. When set,
    /// the file is never read at boot or reload — it only receives
    /// write-backs (self-disable persistence, id materialization). Takes
    /// precedence over [`LoaderConfig::initial`], whose boot-time write it
    /// also suppresses.
    pub document: Option<cordis_include::Document>,
    /// Plugin registry used to resolve entry names; defaults to a fresh
    /// [`PluginRegistry`] with only the `group` builtin.
    pub registry: Option<PluginRegistry>,
    /// Debounce window for coalesced config writes; `None` (default)
    /// persists every write synchronously.
    pub write_debounce: Option<Duration>,
}

impl LoaderConfig {
    /// Configure a loader around `filename`.
    pub fn new(filename: impl Into<PathBuf>) -> Self {
        Self {
            filename: filename.into(),
            initial: None,
            document: None,
            registry: None,
            write_debounce: None,
        }
    }

    /// Provide the document written when the file is missing.
    pub fn with_initial(mut self, initial: cordis_include::Document) -> Self {
        self.initial = Some(initial);
        self
    }

    /// Compose from the given document instead of reading the entry file.
    ///
    /// The loader treats the file as a pure write-back draft: nothing is
    /// read from or written to it at boot, and reloads recompose from this
    /// document (import files are still read). This is the composition
    /// source profile boot needs — the naive "compose → write draft →
    /// open" races between concurrent boots on one profile (another
    /// process's draft could land between this one's write and read) and
    /// requires a writable directory. Replace the source at runtime with
    /// [`Loader::recompose`].
    pub fn with_document(mut self, document: cordis_include::Document) -> Self {
        self.document = Some(document);
        self
    }

    /// Provide the plugin registry entries resolve against.
    pub fn with_registry(mut self, registry: PluginRegistry) -> Self {
        self.registry = Some(registry);
        self
    }

    /// Coalesce config writes: rapid write-backs merge and land once after
    /// this much quiet time.
    pub fn with_write_debounce(mut self, delay: Duration) -> Self {
        self.write_debounce = Some(delay);
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
    /// Serializes the loader's state transitions end to end — `reload`,
    /// `update`, `update_config`, `dispose`, and deferred self-kill
    /// persistence — so file reads, tree diffs, fiber patches, and
    /// write-backs always run in a consistent order. Without it a
    /// watch-thread `reload` can interleave with a plugin-thread
    /// `update_config` and leave the fiber serving a different config than
    /// the tree and the file claim, with no later event to reconcile the
    /// difference. Reentrancy-aware because loader event listeners
    /// legitimately call back into the loader.
    operation: OperationLock,
    /// Composition source override; `None` for file-backed loaders. Set by
    /// [`LoaderConfig::with_document`] and replaced by every
    /// [`Loader::recompose`]: reloads recompose from it instead of re-reading
    /// the root file (import files are still read), so rows a write-back
    /// baked into the draft can never re-enter the composition.
    document: Mutex<Option<cordis_include::Document>>,
    /// Canonical path -> file of every import currently mounted.
    imports: Mutex<HashMap<PathBuf, LoaderFile>>,
    /// Paths already armed by [`Loader::watch`] (watch feature).
    #[cfg(feature = "watch")]
    watched: Mutex<HashSet<PathBuf>>,
    /// Import-file watchers kept alive for hot reload (watch feature).
    #[cfg(feature = "watch")]
    watchers: Mutex<Vec<cordis_include::FileWatcher>>,
    /// Debounce window for coalesced writes; `None` writes synchronously.
    write_debounce: Mutex<Option<Duration>>,
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
        // A document-backed loader never writes its draft at boot either:
        // the root file exists only as a write-back target, so `initial`
        // (a file-backed concern) is ignored entirely.
        if config.document.is_none() && !file.path().exists() {
            if let Some(initial) = &config.initial {
                file.write(initial)?;
            }
        }
        // A corrupt or unreadable main file is fatal — booting an empty
        // loader would silently discard the whole configuration. Import
        // files keep the tolerant record-and-skip path inside `compose`.
        let mut imports = HashMap::new();
        let mut errors = Vec::new();
        let document = config.document;
        let composed = match document.clone() {
            Some(document) => compose_entries(
                document.entries,
                &file,
                &mut imports,
                &mut HashSet::new(),
                &mut HashSet::new(),
                &mut errors,
            ),
            None => compose(
                &file,
                &mut imports,
                &mut HashSet::new(),
                &mut HashSet::new(),
                &mut errors,
            )?,
        };
        let inner = Arc::new(LoaderInner {
            root: root.clone(),
            file,
            tree: EntryTree::new(),
            registry: Mutex::new(config.registry.unwrap_or_default()),
            state: Mutex::new(LoaderState {
                entries: HashMap::new(),
                operating: 0,
                // Every import failure is joined into one message: keeping
                // only the last one hid the rest behind fix-and-retry loops.
                last_error: (!errors.is_empty()).then(|| errors.join("; ")),
                _keep_alive: Vec::new(),
            }),
            operation: OperationLock::default(),
            document: Mutex::new(document),
            imports: Mutex::new(imports),
            #[cfg(feature = "watch")]
            watched: Mutex::new(HashSet::new()),
            #[cfg(feature = "watch")]
            watchers: Mutex::new(Vec::new()),
            write_debounce: Mutex::new(config.write_debounce),
        });
        inner.tree.reconcile(composed)?;
        // Generated ids from the initial load are persisted lazily, on the
        // first explicit write-back.

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
    ///
    /// Every entry resolving to this name shares the one registered
    /// instance (each resolve wraps it in a fresh handle); register a
    /// per-lookup factory with [`register`](Self::register) when entries
    /// need independent instances.
    pub fn register_plugin<P: cordis::Plugin>(&self, plugin: P) {
        lock(&self.inner.registry).register_plugin(plugin);
    }

    /// Register a handle factory under a name.
    ///
    /// The factory runs on every resolve, so it can hand each entry an
    /// independent [`PluginHandle`];
    /// [`register_plugin`](Self::register_plugin) instead shares one
    /// plugin instance across all entries using its name.
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

    /// Set (or clear, with `None`) the debounce window for coalesced
    /// config writes.
    pub fn set_write_debounce(&self, delay: Option<Duration>) {
        *lock(&self.inner.write_debounce) = delay;
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
    /// entries restart under their new parent, redefined entries (plugin
    /// name, inject declaration, or enabled flag changed) stop and start
    /// with their new options, and updated entries are patched in place —
    /// their config-only change never restarts the fiber. A patch the
    /// plugin rejects leaves the fiber on its current config and is
    /// retried by the next reload. Patches are never written back to the
    /// file; only newly generated ids are persisted afterwards. The whole
    /// reconcile runs under the loader's operation lock, serialized
    /// against [`update_config`](Self::update_config) and
    /// [`dispose`](Self::dispose).
    pub fn reload(&self) -> Result<TreeDiff> {
        let inner = &self.inner;
        // Whole-reload exclusion: compose, diff, fiber transitions, and the
        // id write-back must not interleave with recompose() or
        // update_config() or dispose().
        let _operation = inner.operation.guard();
        let mut imports = HashMap::new();
        let mut errors = Vec::new();
        let composed = match lock(&inner.document).clone() {
            // A document-backed loader never re-reads its root file: the
            // file is a write-back draft, and rows a write-back baked into
            // it would re-enter the composition and duplicate every insert.
            Some(document) => compose_entries(
                document.entries,
                &inner.file,
                &mut imports,
                &mut HashSet::new(),
                &mut HashSet::new(),
                &mut errors,
            ),
            None => match compose(
                &inner.file,
                &mut imports,
                &mut HashSet::new(),
                &mut HashSet::new(),
                &mut errors,
            ) {
                Ok(composed) => composed,
                Err(error) => {
                    // The current tree is still the last known-good state. In
                    // particular, do not feed an empty list to `EntryTree`:
                    // that would dispose every running plugin on a transient
                    // parse or I/O failure.
                    self.record_error(&error);
                    return Err(error.into());
                }
            },
        };
        // Id-less rows at any depth — including entries nested inside
        // groups — get a generated id during the tree reconcile below; the
        // write-back must fire for them, or every reload would regenerate
        // a different id and churn the entry's fiber.
        let dirty = missing_id(&composed);
        for error in errors {
            self.record_error(LoaderError::Include(
                cordis_include::IncludeError::Message { message: error },
            ));
        }
        let diff = reconcile(inner, composed, imports)?;

        // Entries created without explicit ids had one generated; persist
        // it to the file that owns them so the next reload can match them.
        if dirty {
            write_back(inner)?;
        }
        #[cfg(feature = "watch")]
        self.arm_import_watchers();
        Ok(diff)
    }

    /// Recompose from a caller-supplied document — the in-memory twin of
    /// [`reload`](Self::reload): the same full reconcile (diff → stop →
    /// patch → start) under the same operation lock, but composed from
    /// `document` instead of any file, and **without write-back**. A
    /// recomposition is not a file edit (upstream's `internal/update`
    /// persists nothing either); ids generated for id-less rows stay in
    /// memory, so those rows restart on every recomposition — the draft is
    /// regenerated anyway.
    ///
    /// The document also becomes the loader's composition source: later
    /// reloads recompose from it instead of re-reading the root file
    /// (import files are still read). This is the core HMR primitive — a
    /// watcher recomposes fresh layers and hands the result to `recompose`.
    pub fn recompose(&self, document: cordis_include::Document) -> Result<TreeDiff> {
        let inner = &self.inner;
        // Same exclusion as reload(): the source swap, tree diff, and fiber
        // transitions must land as one unit.
        let _operation = inner.operation.guard();
        let mut imports = HashMap::new();
        let mut errors = Vec::new();
        let composed = compose_entries(
            document.entries.clone(),
            &inner.file,
            &mut imports,
            &mut HashSet::new(),
            &mut HashSet::new(),
            &mut errors,
        );
        for error in errors {
            self.record_error(LoaderError::Include(
                cordis_include::IncludeError::Message { message: error },
            ));
        }
        *lock(&inner.document) = Some(document);
        let diff = reconcile(inner, composed, imports)?;
        #[cfg(feature = "watch")]
        self.arm_import_watchers();
        Ok(diff)
    }

    /// Change one entry's config at runtime: the fiber is updated (and
    /// restarted when active) via `Fiber::update_value`, the tree entry is
    /// re-committed, and the new config is persisted to the file.
    ///
    /// This is the loader-level entry point of the config-update family;
    /// the fiber-level primitives are `Fiber::update`/`Fiber::update_value`.
    pub fn update_config(&self, id: &str, config: Node) -> Result<()> {
        let inner = &self.inner;
        // Same exclusion as reload(): the fiber transitions, tree commit, and
        // file write-back must land as one unit, or a concurrent reload
        // could patch the fiber back to the file's previous content.
        let _operation = inner.operation.guard();
        let entry = inner.tree.resolve(id).ok_or_else(|| {
            LoaderError::Include(cordis_include::IncludeError::EntryNotFound { id: id.to_owned() })
        })?;
        if let Some(fiber) = entry.fiber() {
            fiber.update_value(Config::new(config.clone()))?;
        }
        let mut options = entry_options_with_children(&entry);
        options.config = Some(config.clone());
        inner
            .tree
            .reconcile_entry(&entry.path(), options, None, None)?;
        write_back(inner)?;
        emit(
            inner,
            crate::events::CONFIG_UPDATE,
            vec![Value::new(entry), Value::new(config)],
        );
        Ok(())
    }

    /// Stop every entry, stop watching files, and release the loader's
    /// root-level effects (the status listener and the `loader` service).
    /// The root context stays usable, and a fresh [`Loader::open`] on the
    /// same root works afterwards.
    pub fn dispose(&self) -> Result<()> {
        let inner = &self.inner;
        // Excluded against reload()/update_config() so entry teardown cannot
        // interleave with a reconcile pass touching the same fibers.
        let _operation = inner.operation.guard();
        for entry in inner.tree.top_level() {
            if let Err(error) = stop_entry(inner, &entry) {
                self.record_error(error);
            }
        }
        #[cfg(feature = "watch")]
        {
            lock(&inner.watched).clear();
            lock(&inner.watchers).clear();
        }
        let keep_alive = std::mem::take(&mut lock(&inner.state)._keep_alive);
        for effect in &keep_alive {
            if let Err(error) = effect.dispose() {
                self.record_error(LoaderError::Cordis(error));
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
        let watcher = self
            .inner
            .file
            .watch(move || {
                if let Err(error) = loader.reload() {
                    loader.record_error(error);
                }
            })
            .map_err(LoaderError::Include)?;
        let main_path = std::fs::canonicalize(self.inner.file.path())
            .unwrap_or_else(|_| self.inner.file.path().to_path_buf());
        lock(&self.inner.watched).insert(main_path);
        self.arm_import_watchers();
        Ok(watcher)
    }

    /// Watch import files that appeared since the last arming; their
    /// watchers live for the loader's lifetime (`watch` feature).
    #[cfg(feature = "watch")]
    fn arm_import_watchers(&self) {
        for (path, file) in lock(&self.inner.imports).clone() {
            if lock(&self.inner.watched).contains(&path) {
                continue;
            }
            let loader = self.clone();
            match file.watch(move || {
                if let Err(error) = loader.reload() {
                    loader.record_error(error);
                }
            }) {
                Ok(watcher) => {
                    lock(&self.inner.watched).insert(path);
                    lock(&self.inner.watchers).push(watcher);
                }
                Err(error) => self.record_error(LoaderError::Include(error)),
            }
        }
    }

    fn record_error(&self, error: impl std::fmt::Display) {
        record_error(&self.inner, &error);
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

/// Reentrancy-aware exclusion for the loader's state transitions.
///
/// Foreign threads block until the current transition finishes; acquisition
/// from the *owning* thread passes through instead of deadlocking. That
/// matters because loader events (`ENTRY_INIT`, `PARTIAL_DISPOSE`, patch
/// events) run listener code inline, and a listener calling back into
/// `reload()`/`update_config()` re-enters on the same thread — a plain
/// `std::sync::Mutex` would deadlock there.
#[derive(Default)]
struct OperationLock {
    state: Mutex<OperationState>,
    released: Condvar,
}

#[derive(Default)]
struct OperationState {
    owner: Option<ThreadId>,
    depth: usize,
}

impl OperationLock {
    /// Acquire the lock, blocking only foreign threads.
    fn guard(&self) -> OperationGuard<'_> {
        let current = std::thread::current().id();
        let mut state = lock(&self.state);
        loop {
            if state.owner.is_none_or(|owner| owner == current) {
                state.owner = Some(current);
                state.depth += 1;
                return OperationGuard { lock: self };
            }
            let guard = self
                .released
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
            state = guard;
        }
    }
}

struct OperationGuard<'a> {
    lock: &'a OperationLock,
}

impl Drop for OperationGuard<'_> {
    fn drop(&mut self) {
        let mut state = lock(&self.lock.state);
        state.depth = state.depth.saturating_sub(1);
        if state.depth == 0 {
            state.owner = None;
            drop(state);
            self.lock.released.notify_all();
        }
    }
}

/// Emit a loader event; listener failures are recorded, never propagated
/// into the state machine.
fn emit(inner: &LoaderInner, name: &str, args: Vec<Value>) {
    if let Err(error) = inner.root.events().emit(name, args) {
        lock(&inner.state).last_error = Some(format!("{name} listener failed: {error}"));
    }
}

/// Record a background error against the loader's state.
fn record_error(inner: &LoaderInner, error: &dyn std::fmt::Display) {
    lock(&inner.state).last_error = Some(error.to_string());
}

/// Apply a freshly composed entry list to tree and fibers: commit the tree
/// diff, stop removed, moved, and redefined subtrees, patch config-only
/// updates in place, start created entries, and restart what was stopped
/// (parents first). Transition errors are recorded and the reconcile
/// continues — the tree is the source of truth and the next pass retries.
fn reconcile(
    inner: &LoaderInner,
    composed: Vec<EntryOptions>,
    imports: HashMap<PathBuf, LoaderFile>,
) -> Result<TreeDiff> {
    let diff = inner.tree.reconcile(composed)?;
    *lock(&inner.imports) = imports;

    for removed in &diff.removed {
        if let Err(error) = stop_entry(inner, &removed.entry) {
            record_error(inner, &error);
        }
    }
    for entry in &diff.moved {
        if let Err(error) = stop_entry(inner, entry) {
            record_error(inner, &error);
        }
    }
    for entry in &diff.redefined {
        if let Err(error) = stop_entry(inner, entry) {
            record_error(inner, &error);
        }
    }
    for entry in &diff.updated {
        if let Err(error) = patch_entry(inner, entry) {
            record_error(inner, &error);
        }
    }
    for entry in &diff.created {
        if let Err(error) = start_entry(inner, entry) {
            record_error(inner, &error);
        }
    }

    // Restart what this pass stopped, parents first so re-parented entries
    // find their group fibers: moved entries under their new parents,
    // redefined entries with their new options, and updated entries that
    // had no live fiber.
    let mut restarts: Vec<&Entry> = diff
        .moved
        .iter()
        .chain(&diff.redefined)
        .chain(diff.updated.iter().filter(|entry| entry.fiber().is_none()))
        .collect();
    restarts.sort_by_key(|entry| entry_depth(entry));
    for entry in restarts {
        if let Err(error) = start_subtree(inner, entry) {
            record_error(inner, &error);
        }
    }
    Ok(diff)
}

/// Start one entry's fiber beneath its parent group's context. The
/// enabled check resolves `!!js` disabled expressions (own slot and every
/// ancestor's); an expression that fails to evaluate is a start failure,
/// recorded by the caller like a resolve failure.
fn start_entry(inner: &LoaderInner, entry: &Entry) -> Result<()> {
    if entry.fiber().is_some() {
        return Ok(());
    }
    if !entry.resolved_enabled()? {
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
    let Some(uid) = fiber.uid() else {
        // The parent context's registry rejected the start (its fiber was
        // disposed concurrently, so the parent-effect registration failed
        // and the new fiber came back with its uid cleared). Recording the
        // rejected fiber here would wedge the entry forever: every later
        // reload sees a fiber and skips the start. Leave the entry
        // unstarted so the next reload retries it, and surface why.
        return Err(LoaderError::Cordis(
            fiber
                .error()
                .unwrap_or_else(|| CordisError::new(ErrorCode::InactiveEffect)),
        ));
    };
    entry.set_fiber(Some(fiber.clone()));
    lock(&inner.state).entries.insert(uid, entry.clone());
    emit(
        inner,
        crate::events::ENTRY_INIT,
        vec![Value::new(entry.clone())],
    );
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

/// Apply a config-only change to a live entry by patching it in place.
/// Structural changes (name, inject, enabled) arrive through
/// `diff.redefined` and never here, so no identity comparison is needed.
/// Entries without a live fiber are left to the reload's restart phase.
/// A `!!js` disabled expression that fails to evaluate propagates the
/// error and leaves the fiber on its current config, retried by the next
/// reload.
fn patch_entry(inner: &LoaderInner, entry: &Entry) -> Result<()> {
    if !entry.resolved_enabled()? {
        return stop_entry(inner, entry);
    }
    let Some(fiber) = entry.fiber() else {
        return Ok(());
    };
    let new_config = entry.resolved_config()?.unwrap_or(Node::Null);
    let current = fiber
        .config()
        .downcast::<Node>()
        .ok()
        .map(|node| (*node).clone());
    if current.as_ref() != Some(&new_config) {
        emit(
            inner,
            crate::events::BEFORE_PATCH,
            vec![Value::new(entry.clone())],
        );
        if let Err(error) = fiber.update_value(Config::new(new_config)) {
            // tree.reconcile() already committed the new options before this
            // patch ran. Rolling the entry's stored config back to what the
            // fiber actually runs keeps the tree honest and — crucially —
            // makes the next reload's diff see a change again, so a config
            // that failed validation is retried instead of silently pinning
            // the fiber to the stale config forever.
            if let Some(old_config) = current {
                let mut options = entry_options_with_children(entry);
                options.config = Some(old_config);
                if let Err(revert) = inner
                    .tree
                    .reconcile_entry(&entry.path(), options, None, None)
                {
                    lock(&inner.state).last_error = Some(format!(
                        "failed to roll back config of {}: {revert}",
                        entry.path()
                    ));
                }
            }
            return Err(LoaderError::Cordis(error));
        }
        emit(
            inner,
            crate::events::AFTER_PATCH,
            vec![Value::new(entry.clone())],
        );
    }
    Ok(())
}

/// (Re)start an entry and its descendants, parents first; `start_entry`
/// itself skips disabled entries and entries that already run.
fn start_subtree(inner: &LoaderInner, entry: &Entry) -> Result<()> {
    start_entry(inner, entry)?;
    for child in entry.children() {
        start_subtree(inner, &child)?;
    }
    Ok(())
}

/// Distance from the tree root, for restarting stopped entries parents
/// first.
fn entry_depth(entry: &Entry) -> usize {
    let mut depth = 0;
    let mut current = entry.clone();
    while let Some(parent) = current.parent() {
        depth += 1;
        current = parent;
    }
    depth
}

/// Serialize an entry together with its live subtree (used by reconcile paths
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

/// Persist the current tree across every involved file, preserving
/// unknown top-level keys. Import subtrees are stripped from their parent
/// file and written to the file they came from.
fn write_back(inner: &LoaderInner) -> Result<()> {
    let mut jobs: Vec<(LoaderFile, Vec<EntryOptions>)> = vec![(
        inner.file.clone(),
        inner
            .tree
            .top_level()
            .iter()
            .map(to_stripped_options)
            .collect(),
    )];
    for entry in inner.tree.entries() {
        if entry.options().import_url().is_some() {
            if let Some(file) = lock(&inner.imports).get(&import_canonical(inner, &entry)) {
                let children = entry.children().iter().map(to_stripped_options).collect();
                jobs.push((file.clone(), children));
            }
        }
    }
    let debounce = *lock(&inner.write_debounce);
    for (file, entries) in jobs {
        let mut document = file.read()?;
        document.entries = entries;
        match debounce {
            Some(delay) => file.write_deferred(document, delay),
            None => file.write(&document)?,
        }
    }
    Ok(())
}

/// The entry's full options with import descendants cut off: an import
/// entry keeps its own fields but drops the children mounted from its
/// file, at any depth.
fn to_stripped_options(entry: &Entry) -> EntryOptions {
    fn strip(options: &mut EntryOptions) {
        if options.import_url().is_some() {
            // Everything below an import comes from its own file.
            options.group.clear();
            return;
        }
        options.group.retain(|child| child.import_url().is_none());
        for child in &mut options.group {
            strip(child);
        }
    }
    let mut options = entry_options_with_children(entry);
    strip(&mut options);
    options
}

/// Resolve an import url against the directory of the file that contains
/// the import entry.
fn import_path(base_file: &LoaderFile, url: &str) -> PathBuf {
    let direct = Path::new(url);
    if direct.is_absolute() {
        return direct.to_path_buf();
    }
    match base_file.path().parent() {
        Some(parent) => parent.join(url),
        None => direct.to_path_buf(),
    }
}

/// The canonical path under which an import entry's file is registered.
fn import_canonical(inner: &LoaderInner, entry: &Entry) -> PathBuf {
    let url = entry.options().import_url().unwrap_or_default().to_owned();
    let path = import_path(&inner.file, &url);
    std::fs::canonicalize(&path).unwrap_or(path)
}

/// Whether any entry in the list — at any nesting depth — lacks an
/// explicit id. `EntryTree::reconcile` generates one for each, and the file
/// must be written back afterwards or the next reload matches nothing and
/// churns those entries' fibers.
fn missing_id(entries: &[EntryOptions]) -> bool {
    entries
        .iter()
        .any(|options| options.id.is_none() || missing_id(&options.group))
}

/// Read `file` and recursively mount import subtrees: every `import`
/// entry's `group` becomes the entries of the file its `url` names, so one
/// `EntryTree::reconcile` diffs across all files uniformly. Returns the
/// composed top-level entries (import children included, so id-less
/// detection sees the whole tree).
///
/// Reading the file passed directly to this call is not recoverable here:
/// callers loading the main file propagate the error, while callers
/// mounting an import catch it above themselves and retain the tolerant
/// skip path.
fn compose(
    file: &LoaderFile,
    imports: &mut HashMap<PathBuf, LoaderFile>,
    active: &mut HashSet<PathBuf>,
    mounted: &mut HashSet<PathBuf>,
    errors: &mut Vec<String>,
) -> cordis_include::Result<Vec<EntryOptions>> {
    let document = file.read()?;
    Ok(compose_entries(
        document.entries,
        file,
        imports,
        active,
        mounted,
        errors,
    ))
}

/// Mount import subtrees under base rows supplied in memory — the entries
/// half of [`compose`], used by document-backed composition sources
/// ([`LoaderConfig::with_document`], [`Loader::recompose`]). `base` resolves
/// relative import urls. Infallible: import failures follow the tolerant
/// record-and-skip path.
///
/// `active` holds the files on the current import chain (cycle detection);
/// `mounted` holds every file mounted anywhere in this compose. The entry
/// tree keys entries by globally unique id, so the import graph must be a
/// tree: real cycles and diamonds (the same file mounted twice) are both
/// reported and their reference dropped, but with distinct diagnoses.
fn compose_entries(
    entries: Vec<EntryOptions>,
    base: &LoaderFile,
    imports: &mut HashMap<PathBuf, LoaderFile>,
    active: &mut HashSet<PathBuf>,
    mounted: &mut HashSet<PathBuf>,
    errors: &mut Vec<String>,
) -> Vec<EntryOptions> {
    let mut composed = Vec::with_capacity(entries.len());
    for mut options in entries {
        if let Some(url) = options.import_url().map(str::to_owned) {
            let path = import_path(base, &url);
            let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if !active.insert(canonical.clone()) {
                errors.push(format!("import cycle detected at {}", path.display()));
                // Drop the cyclic reference: keeping a copy of the entry
                // would duplicate its id inside the composed tree.
                continue;
            }
            if !mounted.insert(canonical.clone()) {
                errors.push(format!(
                    "duplicate import: {} is already mounted elsewhere; \
                     the import graph must be a tree",
                    path.display()
                ));
                active.remove(&canonical);
                continue;
            }
            match LoaderFile::open(&path) {
                Ok(sub_file) => {
                    match compose(&sub_file, imports, active, mounted, errors) {
                        Ok(sub_entries) => {
                            options.group = sub_entries;
                        }
                        Err(error) => {
                            errors.push(format!(
                                "cannot read import {}: {error}",
                                sub_file.path().display()
                            ));
                            // Never trust children embedded in an import
                            // marker when its owning file could not be read.
                            options.group.clear();
                        }
                    }
                    imports.insert(canonical.clone(), sub_file);
                }
                Err(error) => errors.push(format!(
                    "cannot open import {} ({}: {error})",
                    path.display(),
                    base.path().display()
                )),
            }
            // A file is "active" only while its own subtree composes, so
            // sibling imports of different files never look like cycles.
            active.remove(&canonical);
        }
        composed.push(options);
    }
    composed
}

/// Route `internal/status` disposals: a fiber that reached `Disposed`
/// outside loader operation was killed by its own plugin, so record
/// `disabled: true` in the tree and persist it.
///
/// The status event fires while the dying fiber still holds its transition
/// mutex, so the persistence itself — a tree mutation, file serialize +
/// fsync + rename, and `PARTIAL_DISPOSE` listeners — is deferred to a
/// short-lived thread. Running it inline would stretch that critical
/// section across disk I/O and arbitrary user code, making other threads'
/// restart/dispose on the same fiber time out on stalls that have nothing
/// to do with the fiber's own teardown.
fn handle_status(inner: &Arc<LoaderInner>, event: &cordis::Event) -> cordis::EventResult {
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
    let deferred = std::thread::Builder::new()
        .name("cordis-self-dispose".to_owned())
        .spawn({
            // A strong reference keeps the loader alive until the record
            // lands, even if the caller drops every Loader handle at once.
            let inner = Arc::clone(inner);
            let entry = entry.clone();
            move || {
                // Serialized with reload()/update_config()/dispose() so the
                // self-kill write-back cannot interleave with a reconcile.
                let _operation = inner.operation.guard();
                if let Err(error) = persist_self_dispose(&inner, &entry) {
                    lock(&inner.state).last_error = Some(error.to_string());
                }
            }
        });
    match deferred {
        Ok(_join) => {}
        Err(_) => {
            // Could not spawn a thread: persist inline rather than losing
            // the self-kill record.
            let _operation = inner.operation.guard();
            if let Err(error) = persist_self_dispose(inner, &entry) {
                lock(&inner.state).last_error = Some(error.to_string());
            }
        }
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
    // The static flag overwrites any `!!js` expression the slot held.
    // Upstream keeps the raw expression in the options; this port trades
    // that for the dead entry's final state — the draft is regenerated
    // (and the expression restored) on every recomposition anyway.
    options.disabled = cordis_include::Disabled::Flag(true);
    inner
        .tree
        .reconcile_entry(&entry.path(), options, None, None)?;
    write_back(inner)?;
    emit(
        inner,
        crate::events::PARTIAL_DISPOSE,
        vec![Value::new(entry.clone())],
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis::{Inject, PluginOutput, plugin_sync};

    /// Regression (#36): when the parent group's fiber dies before a child
    /// entry starts, the registry rejects the new fiber (uid cleared,
    /// state Disposed). start_entry must surface the rejection and leave
    /// the entry without a fiber — recording the rejected fiber wedged the
    /// entry forever, since every later reload saw a fiber and skipped the
    /// start.
    #[test]
    fn rejected_start_leaves_the_entry_retryable() {
        let path = std::env::temp_dir().join(format!(
            "cordis-loader-rejected-start-{}-{}.yml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_file(&path);
        let mut registry = PluginRegistry::new();
        registry.register("worker", || {
            plugin_sync::<Node, _>("worker", Inject::default(), |_, _| Ok(PluginOutput::none()))
        });
        let root = Context::new();
        let loader = Loader::open(
            &root,
            LoaderConfig::new(&path)
                .with_registry(registry)
                .with_initial(cordis_include::Document::with_entries(vec![
                    EntryOptions::new("group")
                        .with_id("g1")
                        .with_group(vec![EntryOptions::new("worker").with_id("c1")]),
                ])),
        )
        .unwrap();
        let inner = &loader.inner;
        let group = inner.tree.resolve("g1").unwrap();
        let child = inner.tree.resolve("g1:c1").unwrap();
        assert!(group.fiber().is_some() && child.fiber().is_some());

        // Kill the parent group while the loader looks away (no self-kill
        // bookkeeping), then model the child as not-yet-started.
        {
            let _operating = OperatingGuard::new(&inner.state);
            group.fiber().unwrap().dispose().unwrap();
        }
        child.set_fiber(None);

        let result = start_entry(inner, &child);
        assert!(result.is_err(), "the registry rejection must surface");
        assert!(child.fiber().is_none(), "no rejected fiber recorded");

        // Still eligible: retrying fails the same way instead of silently
        // doing nothing because a dead fiber occupies the entry.
        assert!(start_entry(inner, &child).is_err());
        assert!(child.fiber().is_none());

        drop(loader);
        let _ = std::fs::remove_file(&path);
    }
}
