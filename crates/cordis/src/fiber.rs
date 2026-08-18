//! Plugin fiber lifecycle, dependency epochs, and effect cleanup.

use crate::context::{Context, ContextMeta, Isolation, RootInner};
use crate::effect::{AsyncDisposer, EffectCell, EffectHandle, EffectMeta};
use crate::registry::{Inject, PluginHandle, PluginKey};
use crate::utils::{block_on, lock};
use crate::{Config, CordisError, ErrorCode, Result, Value};
use std::fmt::{self, Debug, Formatter};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// Plugin lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FiberState {
    /// Waiting for one or more required services.
    Pending,
    /// Plugin callback or validator is running.
    Loading,
    /// Loaded and providing services.
    Active,
    /// Validation or startup failed.
    Failed,
    /// Permanently removed from its parent and registry.
    Disposed,
    /// Effect cleanup is running.
    Unloading,
}

struct FiberData {
    state: FiberState,
    raw_config: Config,
    config: Config,
    /// Validated config stashed by update_value's pre-check, keyed by the raw
    /// config's Arc identity so activate can skip re-validation.
    validated: Option<(Config, Config)>,
    error: Option<CordisError>,
    active_epoch: Option<Vec<u64>>,
    failed_epoch: Option<Vec<u64>>,
}

pub(crate) struct FiberInner {
    root: Weak<RootInner>,
    uid: Mutex<Option<u64>>,
    parent: Weak<FiberInner>,
    meta: ContextMeta,
    plugin: Option<PluginHandle>,
    inject: Inject,
    data: Mutex<FiberData>,
    effects: Mutex<Vec<Arc<EffectCell>>>,
    parent_effect: Mutex<Option<EffectHandle>>,
    transition: Mutex<()>,
    dirty: AtomicBool,
}

impl FiberInner {
    pub(crate) fn uid_value(&self) -> Option<u64> {
        *lock(&self.uid)
    }

    pub(crate) fn remove_effect(&self, id: u64) {
        lock(&self.effects).retain(|effect| effect.id != id);
    }
}

// Fibers whose transition mutex the current thread holds, keyed by
// `FiberInner` address. Presence means "called from inside a lifecycle
// callback on this fiber" — reentrancy, which must fail fast rather than
// deadlock.
thread_local! {
    static HELD_TRANSITIONS: std::cell::RefCell<std::collections::HashSet<usize>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
}

/// A transition-mutex guard that unregisters its fiber from
/// [`HELD_TRANSITIONS`] when dropped, including on unwind.
struct TransitionGuard<'a> {
    /// Held purely for the lock; the guard itself is never touched.
    _guard: std::sync::MutexGuard<'a, ()>,
    key: usize,
}

impl Drop for TransitionGuard<'_> {
    fn drop(&mut self) {
        HELD_TRANSITIONS.with(|held| held.borrow_mut().remove(&self.key));
    }
}

/// Ceiling on how long `restart`/`dispose` wait for a transition held by
/// another thread before failing. Generous on purpose: plugin `apply`s and
/// disposer chains legitimately run under the transition mutex too.
const TRANSITION_WAIT: Duration = Duration::from_secs(10);

/// Test-only millisecond override for [`TRANSITION_WAIT`]; zero keeps the
/// default so timeout regressions do not need a real ten-second stall.
#[cfg(test)]
static TRANSITION_WAIT_MILLIS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn transition_wait() -> Duration {
    #[cfg(test)]
    {
        let millis = TRANSITION_WAIT_MILLIS.load(Ordering::Relaxed);
        if millis != 0 {
            return Duration::from_millis(millis);
        }
    }
    TRANSITION_WAIT
}

/// Cloneable handle to one plugin lifecycle instance.
#[derive(Clone)]
pub struct Fiber {
    pub(crate) inner: Arc<FiberInner>,
}

/// Test-only hook armed by regression tests to stretch the lost-wakeup
/// window in [`Fiber::refresh`] — the few instructions between the final
/// dirty-flag check and the transition lock release — so a notification can
/// be landed inside it deterministically.
#[cfg(test)]
static REFRESH_WINDOW_STRETCH: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
fn stretch_refresh_window() {
    if REFRESH_WINDOW_STRETCH.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

impl Fiber {
    pub(crate) fn from_inner(inner: Arc<FiberInner>) -> Self {
        Self { inner }
    }

    pub(crate) fn new_root(root: Weak<RootInner>, meta: ContextMeta) -> Self {
        Self {
            inner: Arc::new(FiberInner {
                root,
                uid: Mutex::new(Some(0)),
                parent: Weak::new(),
                meta,
                plugin: None,
                inject: Inject::default(),
                data: Mutex::new(FiberData {
                    state: FiberState::Active,
                    raw_config: Config::default(),
                    config: Config::default(),
                    validated: None,
                    error: None,
                    active_epoch: Some(Vec::new()),
                    failed_epoch: None,
                }),
                effects: Mutex::new(Vec::new()),
                parent_effect: Mutex::new(None),
                transition: Mutex::new(()),
                dirty: AtomicBool::new(false),
            }),
        }
    }

    pub(crate) fn new_plugin(parent_ctx: &Context, plugin: PluginHandle, config: Config) -> Self {
        let inject = plugin.plugin().inject().clone();
        let mut meta = parent_ctx.meta.clone();
        if !inject.is_empty() {
            let mut intercepts = (*meta.intercepts).clone();
            for dependency in inject.iter() {
                if let Some(config) = dependency.config.clone() {
                    intercepts.push((dependency.name.clone(), config));
                }
            }
            meta.intercepts = Arc::new(intercepts);
        }
        let parent = parent_ctx.fiber().ok();
        Self {
            inner: Arc::new(FiberInner {
                root: Arc::downgrade(parent_ctx.root_arc()),
                uid: Mutex::new(Some(parent_ctx.root.fiber_id())),
                parent: parent
                    .as_ref()
                    .map(|fiber| Arc::downgrade(&fiber.inner))
                    .unwrap_or_default(),
                meta,
                plugin: Some(plugin),
                inject,
                data: Mutex::new(FiberData {
                    state: FiberState::Pending,
                    raw_config: config.clone(),
                    config,
                    validated: None,
                    error: None,
                    active_epoch: None,
                    failed_epoch: None,
                }),
                effects: Mutex::new(Vec::new()),
                parent_effect: Mutex::new(None),
                transition: Mutex::new(()),
                dirty: AtomicBool::new(false),
            }),
        }
    }

    /// Unique id within the root registry, or `None` once disposed.
    pub fn uid(&self) -> Option<u64> {
        self.inner.uid_value()
    }

    /// Whether both handles refer to the same fiber instance.
    ///
    /// Unlike comparing [`uid`](Self::uid), this stays valid after disposal
    /// (uids are cleared then), which is what "which entry owned this
    /// fiber?" lookups need.
    pub fn ptr_eq(&self, other: &Fiber) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// Current lifecycle state.
    pub fn state(&self) -> FiberState {
        lock(&self.inner.data).state
    }

    /// Plugin display name, inherited from a named parent when needed.
    pub fn name(&self) -> String {
        if let Some(plugin) = self.inner.plugin.as_ref() {
            let name = plugin.name();
            if !name.is_empty() && name != "apply" && name != "anonymous" {
                return name.to_owned();
            }
        }
        if let Some(parent) = self.inner.parent.upgrade() {
            return Fiber::from_inner(parent).name();
        }
        "root".to_owned()
    }

    /// Stable callback identity, absent for the root fiber.
    pub fn plugin_key(&self) -> Option<PluginKey> {
        self.inner.plugin.as_ref().map(PluginHandle::key)
    }

    /// Context in which this plugin runs.
    ///
    /// Returns `None` when every `Context` of the application has been
    /// dropped and only fiber handles remain: without a live root there is
    /// nothing to bind the context to. Lifecycle operations tolerate that
    /// situation; callers holding the last context should drop fibers first.
    pub fn context(&self) -> Option<Context> {
        Some(Context {
            root: self.inner.root.upgrade()?,
            fiber: Arc::downgrade(&self.inner),
            meta: self.inner.meta.clone(),
        })
    }

    /// Normalized service dependency declaration.
    pub fn inject(&self) -> &Inject {
        &self.inner.inject
    }

    /// Isolation override for `name` from this fiber's own metadata, without
    /// constructing a context or touching the reflect lock.
    pub(crate) fn scope_override(&self, name: &str) -> Option<Isolation> {
        self.inner.meta.isolates.get(name).copied()
    }

    /// Validated config from the latest successful activation.
    pub fn config(&self) -> Config {
        lock(&self.inner.data).config.clone()
    }

    /// Last plugin startup error.
    pub fn error(&self) -> Option<CordisError> {
        lock(&self.inner.data).error.clone()
    }

    /// Throw when the fiber has already been disposed.
    pub fn assert_active(&self) -> Result<()> {
        if self.uid().is_none() || self.state() == FiberState::Disposed {
            Err(CordisError::new(ErrorCode::InactiveEffect))
        } else {
            Ok(())
        }
    }

    /// Register a boxed cleanup operation.
    ///
    /// Registration is serialized against disposal: `dispose`/`restart`
    /// mark the fiber dead (uid cleared or `Unloading`) *before* draining
    /// the effect list, so re-checking liveness while holding the list lock
    /// guarantees every accepted effect is seen by a concurrent drain. An
    /// effect that lands after the drain is undone on the spot instead of
    /// leaking with a disposer that never runs.
    pub fn register_effect(
        &self,
        label: impl Into<String>,
        disposer: AsyncDisposer,
    ) -> Result<EffectHandle> {
        self.assert_active()?;
        if self.state() == FiberState::Unloading {
            return Err(CordisError::new(ErrorCode::InactiveEffect));
        }
        let root = self
            .inner
            .root
            .upgrade()
            .ok_or_else(|| CordisError::new(ErrorCode::InactiveEffect))?;
        let cell = EffectCell::new(
            root.effect_id(),
            Arc::downgrade(&self.inner),
            label,
            disposer,
        );
        {
            let mut effects = lock(&self.inner.effects);
            effects.push(cell.clone());
            let dead = self.uid().is_none()
                || matches!(self.state(), FiberState::Disposed | FiberState::Unloading);
            if dead {
                // The drain already ran (or marked the fiber dead and will
                // not see this push after the removal): undo the
                // registration atomically instead of leaking it.
                effects.retain(|effect| effect.id != cell.id);
                drop(effects);
                cell.cancel();
                return Err(CordisError::new(ErrorCode::InactiveEffect));
            }
        }
        Ok(EffectHandle::new(cell))
    }

    /// Register a synchronous cleanup callback.
    pub fn effect<F>(&self, label: impl Into<String>, disposer: F) -> Result<EffectHandle>
    where
        F: FnOnce() -> Result<()> + Send + 'static,
    {
        self.register_effect(label, AsyncDisposer::from_sync(disposer))
    }

    /// Metadata for all currently live top-level effects.
    pub fn effects(&self) -> Vec<EffectMeta> {
        lock(&self.inner.effects)
            .iter()
            .map(|effect| EffectHandle::new(effect.clone()).meta())
            .collect()
    }

    pub(crate) fn set_parent_effect(&self, effect: EffectHandle) {
        *lock(&self.inner.parent_effect) = Some(effect);
    }

    pub(crate) fn reject(&self, error: CordisError) {
        // A rejected fiber never starts, so nothing else will remove the
        // registry record pushed before its parent-effect registration
        // failed. Remove it here or len()/contains() misreport forever.
        let uid = lock(&self.inner.uid).take();
        if let (Some(root), Some(key), Some(uid)) =
            (self.inner.root.upgrade(), self.plugin_key(), uid)
        {
            root.registry.remove_fiber(key, uid);
        }
        let mut data = lock(&self.inner.data);
        data.error = Some(error);
        data.state = FiberState::Disposed;
    }

    fn set_state(&self, state: FiberState) {
        let old = {
            let mut data = lock(&self.inner.data);
            let old = data.state;
            if old == state {
                return;
            }
            data.state = state;
            old
        };

        // The root may be gone when only fiber handles remain; the state
        // still changes, but notifications need somewhere to go.
        if let Some(ctx) = self.context() {
            if let Err(error) = ctx.events().emit(
                "internal/status",
                [Value::new(self.clone()), Value::new(old)],
            ) {
                ctx.log_error(error);
            }
        }

        if old == FiberState::Active || state == FiberState::Active {
            if let Some(root) = self.inner.root.upgrade() {
                root.notify_fiber_services(self);
            }
        }
    }

    fn dispose_effects(&self) {
        let effects = {
            let mut effects = lock(&self.inner.effects);
            std::mem::take(&mut *effects)
        };
        for effect in effects.into_iter().rev() {
            if let Err(error) = EffectHandle::new(effect).dispose() {
                if let Some(ctx) = self.context() {
                    ctx.log_error(error);
                }
            }
        }
    }

    fn dependency_epoch(&self) -> Option<Vec<u64>> {
        let root = self.inner.root.upgrade()?;
        root.dependency_epoch(&self.context()?, &self.inner.inject)
    }

    fn activate(&self, epoch: Vec<u64>) {
        let Some(ctx) = self.context() else {
            // The root vanished between the dependency check and this
            // activation; without it no dependency can stay resolved, so
            // fall back to Pending instead of panicking.
            self.set_state(FiberState::Pending);
            return;
        };
        self.set_state(FiberState::Loading);

        let plugin = self.inner.plugin.as_ref().expect("plugin fiber").clone();
        let (raw_config, validated) = {
            let mut data = lock(&self.inner.data);
            let raw_config = data.raw_config.clone();
            // Consume the update_value pre-validation only when it belongs to
            // this exact raw config allocation.
            let validated = match data.validated.take() {
                Some((raw, config)) if raw.ptr_eq(&raw_config) => Some(config),
                _ => None,
            };
            (raw_config, validated)
        };
        let result = match validated {
            Some(config) => Ok(config),
            None => plugin.plugin().validate_config(raw_config),
        }
        .and_then(|config| {
            let output = block_on(plugin.plugin().apply(ctx, config.clone()))?;
            for (label, disposer) in output.disposers {
                self.register_effect(label, disposer)?;
            }
            lock(&self.inner.data).config = config;
            Ok(())
        });

        match result {
            Ok(()) => {
                {
                    let mut data = lock(&self.inner.data);
                    data.error = None;
                    data.failed_epoch = None;
                    data.active_epoch = Some(epoch);
                }
                self.set_state(FiberState::Active);
            }
            Err(error) => {
                if let Some(ctx) = self.context() {
                    ctx.log_error(&error);
                }
                self.set_state(FiberState::Unloading);
                self.dispose_effects();
                {
                    let mut data = lock(&self.inner.data);
                    data.active_epoch = None;
                    data.failed_epoch = Some(epoch);
                    data.error = Some(error);
                }
                self.set_state(FiberState::Failed);
            }
        }
    }

    fn unload_to(&self, target: FiberState) {
        let has_work =
            !lock(&self.inner.effects).is_empty() || lock(&self.inner.data).active_epoch.is_some();
        if has_work || self.state() == FiberState::Active || self.state() == FiberState::Loading {
            self.set_state(FiberState::Unloading);
            self.dispose_effects();
        }
        {
            let mut data = lock(&self.inner.data);
            data.active_epoch = None;
            if target == FiberState::Pending {
                data.error = None;
            }
        }
        self.set_state(target);
    }

    fn reconcile(&self) {
        if self.uid().is_none() || self.inner.plugin.is_none() {
            return;
        }
        let desired = self.dependency_epoch();
        enum Then {
            Stay,
            SetPending,
            Activate,
        }
        // Decide under one data lock instead of cloning both epoch snapshots.
        let (unload, then) = {
            let data = lock(&self.inner.data);
            match desired.as_ref() {
                None => {
                    if data.active_epoch.is_some()
                        || matches!(
                            data.state,
                            FiberState::Active | FiberState::Loading | FiberState::Failed
                        )
                    {
                        (true, Then::SetPending)
                    } else if data.state != FiberState::Pending {
                        (false, Then::SetPending)
                    } else {
                        (false, Then::Stay)
                    }
                }
                Some(epoch) => {
                    let unchanged = (data.active_epoch.as_ref() == Some(epoch)
                        && data.state == FiberState::Active)
                        || (data.failed_epoch.as_ref() == Some(epoch)
                            && data.state == FiberState::Failed);
                    if unchanged {
                        (false, Then::Stay)
                    } else {
                        (
                            data.active_epoch.is_some() || data.state == FiberState::Active,
                            Then::Activate,
                        )
                    }
                }
            }
        };
        if unload {
            self.unload_to(FiberState::Pending);
        }
        match then {
            Then::Stay => {}
            Then::SetPending => {
                if !unload {
                    self.set_state(FiberState::Pending);
                }
            }
            Then::Activate => {
                if let Some(epoch) = desired {
                    self.activate(epoch);
                }
            }
        }
    }

    pub(crate) fn refresh(&self) {
        loop {
            self.inner.dirty.store(true, Ordering::Release);
            let guard = match self.inner.transition.try_lock() {
                Ok(guard) => guard,
                Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
                // Another thread holds the transition lock. Whoever releases
                // it re-checks the dirty flag below and drains on our behalf,
                // so deferring to the holder loses nothing.
                Err(std::sync::TryLockError::WouldBlock) => return,
            };
            while self.inner.dirty.swap(false, Ordering::AcqRel) {
                self.reconcile();
            }
            #[cfg(test)]
            stretch_refresh_window();
            // A notification landing between the final swap above and this
            // drop only sets the dirty flag: its own try_lock fails, so no
            // one would consume it. Re-check after releasing the lock and
            // take another turn instead of losing the wakeup.
            drop(guard);
            if !self.inner.dirty.load(Ordering::Acquire) {
                return;
            }
        }
    }

    /// Acquire the transition mutex, giving up once [`transition_wait`]
    /// elapses while another thread holds it.
    ///
    /// The std mutex has no timed lock, so contention resolves through a
    /// try-lock poll with exponential backoff; the interval ceilings keep
    /// the polling cost negligible next to the transitions being waited out.
    fn acquire_transition(&self) -> Option<std::sync::MutexGuard<'_, ()>> {
        let deadline = Instant::now() + transition_wait();
        let mut backoff = Duration::from_micros(100);
        loop {
            match self.inner.transition.try_lock() {
                Ok(guard) => return Some(guard),
                Err(std::sync::TryLockError::Poisoned(error)) => return Some(error.into_inner()),
                Err(std::sync::TryLockError::WouldBlock) => {}
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            std::thread::sleep(backoff.min(deadline - now));
            backoff = (backoff * 2).min(Duration::from_millis(5));
        }
    }

    /// Lock the transition mutex, telling reentrancy apart from contention.
    ///
    /// When the *current thread* already holds the lock — directly or
    /// through a lifecycle callback such as an `internal/status` listener
    /// or a disposer — the call is reentrant and fails fast instead of
    /// deadlocking; [`refresh`](Self::refresh) degrades to a dirty flag in
    /// that situation, while `restart`/`dispose` have no deferrable
    /// semantics and report the reentrancy as an error. When a *different
    /// thread* holds the lock, the call waits for the in-flight transition
    /// to finish instead of silently dropping the operation — but only up
    /// to [`transition_wait`]: past that it fails with (and logs) an error
    /// naming the fiber, so a hung `apply` or disposer surfaces instead of
    /// stalling the caller forever.
    fn try_transition(&self) -> Result<TransitionGuard<'_>> {
        let key = Arc::as_ptr(&self.inner) as usize;
        if HELD_TRANSITIONS.with(|held| held.borrow().contains(&key)) {
            return Err(CordisError::with_message(
                ErrorCode::Other,
                "lifecycle transition already in progress on this fiber",
            ));
        }
        let Some(guard) = self.acquire_transition() else {
            let name = self.name();
            let uid = self
                .uid()
                .map_or_else(|| "?".to_owned(), |uid| uid.to_string());
            let error = CordisError::with_message(
                ErrorCode::Other,
                format!(
                    "timed out waiting for the in-flight lifecycle transition \
                     on fiber {name} (uid {uid}); its apply or a disposer may be hung",
                ),
            );
            // The caller only sees the failure; the log records which fiber
            // is stuck so the hung plugin can be identified.
            if let Some(ctx) = self.context() {
                ctx.log_error(&error);
            }
            return Err(error);
        };
        HELD_TRANSITIONS.with(|held| held.borrow_mut().insert(key));
        Ok(TransitionGuard { _guard: guard, key })
    }

    /// Report the fiber's settled lifecycle state without blocking.
    ///
    /// Despite the upstream name, this port drives transitions eagerly, so
    /// there is nothing to wait for: the method polls the current state and
    /// never suspends. `Ok` is returned only when `Active`; a startup failure
    /// is rethrown; `Pending` (dependencies missing), in-flight, and
    /// `Disposed` fibers yield an error rather than a false success. A
    /// `Pending` plugin fiber names the injected services that have not
    /// resolved yet in its error, when the probe can run. Safe to call from
    /// lifecycle callbacks.
    pub fn try_wait(&self) -> Result<Fiber> {
        match self.state() {
            FiberState::Active => Ok(self.clone()),
            FiberState::Failed => Err(self
                .error()
                .unwrap_or_else(|| CordisError::new(ErrorCode::Plugin))),
            FiberState::Pending => {
                // Root fibers have an empty inject and every Context dropped
                // leaves no root to probe; both fall back to the plain message.
                // The probe holds the reflect state lock only briefly — the
                // same one reconcile's dependency_epoch takes — so this stays
                // callable from lifecycle callbacks.
                let missing = match (self.inner.plugin.is_some(), self.inner.root.upgrade()) {
                    (true, Some(root)) => match self.context() {
                        Some(ctx) => root.missing_dependencies(&ctx, &self.inner.inject),
                        None => Vec::new(),
                    },
                    _ => Vec::new(),
                };
                let message = if missing.is_empty() {
                    "fiber is not ready (state: Pending)".to_owned()
                } else {
                    format!(
                        "fiber is not ready (state: Pending; missing services: {})",
                        missing.join(", ")
                    )
                };
                Err(CordisError::with_message(ErrorCode::Other, message))
            }
            state => Err(CordisError::with_message(
                ErrorCode::Other,
                format!("fiber is not ready (state: {state:?})"),
            )),
        }
    }

    /// Block until no lifecycle transition is in progress on this fiber.
    ///
    /// Never call this from a lifecycle callback (status listener, disposer,
    /// plugin apply) on the same thread: the transition mutex is held while
    /// those run, and blocking on it here would deadlock.
    pub fn await_idle(&self) {
        {
            let _guard = lock(&self.inner.transition);
        }
        // A concurrent refresh whose try_lock lost to the guard above only
        // set the dirty flag. Unlike refresh/restart/dispose, the guard here
        // protects no reconcile pass, so drain the flag explicitly or the
        // fiber would stay stale until the next event.
        if self.inner.dirty.load(Ordering::Acquire) {
            self.refresh();
        }
    }

    /// Whether no lifecycle transition is in progress on this fiber.
    ///
    /// The non-blocking counterpart of [`await_idle`](Self::await_idle):
    /// lifecycle callbacks and plugin `apply` run on the fiber's own thread
    /// with the transition mutex held, so they can probe `idle()` safely but
    /// must never block on `await_idle`. This is a pure probe — it does not
    /// drain the dirty flag or trigger a reconcile.
    pub fn idle(&self) -> bool {
        matches!(
            self.inner.transition.try_lock(),
            Ok(_) | Err(std::sync::TryLockError::Poisoned(_))
        )
    }

    /// Async equivalent of [`try_wait`](Self::try_wait).
    ///
    /// **This never suspends.** It is a synchronous poll despite the
    /// `async` signature — the eager lifecycle model means there is nothing
    /// to wait for — so awaiting it runs to completion without ever
    /// yielding to the executor.
    pub async fn await_ready(&self) -> Result<Fiber> {
        self.try_wait()
    }

    /// Dispose and immediately reload this plugin with its current config.
    ///
    /// When another thread is mid-transition on this fiber (plugin `apply`,
    /// disposers, a concurrent `restart`/`dispose`), the call waits for it
    /// to finish — bounded by a ceiling (currently ten seconds) so a hung
    /// transition fails with an error naming the fiber instead of blocking
    /// the caller forever.
    ///
    /// # Deadlock warning
    ///
    /// Calling this on a *different* fiber from inside a lifecycle callback
    /// (disposer, status listener, plugin `apply`) can stall that callback
    /// for the full bounded wait: two fibers whose callbacks target each
    /// other concurrently both time out rather than complete. Trigger
    /// cross-fiber lifecycle operations from a plain thread instead.
    pub fn restart(&self) -> Result<()> {
        self.assert_active()?;
        if self.inner.plugin.is_none() {
            return Err(CordisError::with_message(
                ErrorCode::Other,
                "cannot restart the root fiber",
            ));
        }
        {
            let _guard = self.try_transition()?;
            {
                let mut data = lock(&self.inner.data);
                data.failed_epoch = None;
                data.error = None;
            }
            self.unload_to(FiberState::Pending);
        }
        self.refresh();
        self.try_wait().map(|_| ())
    }

    /// Validate and apply new config, then restart when dependencies are active.
    ///
    /// An `Active` fiber is validated first — a validation failure keeps the
    /// running plugin untouched — then restarted, so the returned result
    /// reflects the new startup.
    ///
    /// A `Pending` or `Failed` fiber instead stores the new config, clears
    /// any previous startup error, and reconciles: it activates with the new
    /// config once its dependencies are (or become) available. Matching
    /// upstream Cordis, `Ok(())` then only means the config was accepted, not
    /// that startup succeeded; inspect [`state`](Self::state) or call
    /// [`try_wait`](Self::try_wait) when the outcome matters.
    pub fn update<C>(&self, config: C) -> Result<()>
    where
        C: Send + Sync + 'static,
    {
        self.update_value(Config::new(config))
    }

    /// Type-erased variant of [`update`](Self::update).
    pub fn update_value(&self, config: Config) -> Result<()> {
        self.assert_active()?;
        let Some(plugin) = self.inner.plugin.as_ref() else {
            return Err(CordisError::with_message(
                ErrorCode::Other,
                "cannot update config on the root fiber",
            ));
        };
        // Match Cordis: validation happens before an active plugin is torn
        // down. The validated result is stashed so activate() does not run
        // the validator twice for the same raw config.
        let validated = if self.state() == FiberState::Active {
            Some(plugin.plugin().validate_config(config.clone())?)
        } else {
            None
        };
        {
            let mut data = lock(&self.inner.data);
            data.validated = validated.map(|valid| (config.clone(), valid));
            data.raw_config = config;
            data.failed_epoch = None;
            data.error = None;
        }
        if self.state() == FiberState::Active {
            self.restart()
        } else {
            // Match Cordis: a config update on an inactive fiber is stored
            // and reconciled, not awaited. The refresh above may already
            // have re-run a failed startup; the outcome is observable
            // through state()/error() rather than this return value.
            self.refresh();
            Ok(())
        }
    }

    /// Permanently dispose this plugin fiber. Repeated calls are no-ops.
    ///
    /// A dispose from another thread waits for an in-flight transition on
    /// this fiber (plugin `apply`, disposers, a concurrent `restart`/
    /// `dispose`) to finish, bounded by a ceiling (currently ten seconds):
    /// past it the call fails with an error naming the fiber — and logs it —
    /// instead of blocking forever. A reentrant call from inside a
    /// lifecycle callback on the *same* fiber fails fast instead of
    /// deadlocking.
    ///
    /// # Deadlock warning
    ///
    /// Calling `dispose` on a *different* fiber from inside a lifecycle
    /// callback (disposer, status listener, plugin `apply`) can stall that
    /// callback for the full bounded wait: two fibers whose disposers
    /// target each other concurrently both time out rather than complete.
    /// Trigger cross-fiber teardown from a plain thread instead.
    pub fn dispose(&self) -> Result<()> {
        // Take the transition lock up front: reentrant calls fail fast, no
        // partial teardown is left behind, and concurrent callers queue
        // behind the in-flight transition (up to the wait ceiling).
        let _guard = self.try_transition()?;
        if self.inner.plugin.is_none() {
            // Root disposal unloads all root-owned effects but leaves the root
            // context usable, matching the original root fiber's restart.
            self.set_state(FiberState::Unloading);
            self.dispose_effects();
            self.set_state(FiberState::Active);
            return Ok(());
        }

        let old_uid = {
            let mut uid = lock(&self.inner.uid);
            let Some(value) = *uid else {
                return Ok(());
            };
            *uid = None;
            value
        };

        if let Some(effect) = lock(&self.inner.parent_effect).take() {
            effect.cancel();
        }
        if let (Some(root), Some(key)) = (self.inner.root.upgrade(), self.plugin_key()) {
            root.registry.remove_fiber(key, old_uid);
        }

        if let Some(ctx) = self.context() {
            if let Err(error) = ctx
                .events()
                .emit("internal/plugin", [Value::new(self.clone())])
            {
                ctx.log_error(error);
            }
        }

        self.unload_to(FiberState::Disposed);
        Ok(())
    }

    /// Async equivalent of [`dispose`](Self::dispose).
    ///
    /// **This is a synchronous pass-through, not an offload.** Awaiting it
    /// runs the whole disposal chain — disposers included — on the calling
    /// thread before the future resolves; it never yields to the executor.
    /// Inside an async runtime, treat `.await`ing this like calling any
    /// blocking function: a slow disposer stalls the executor thread for
    /// the duration. Trigger disposals of long-teardown fibers from a plain
    /// thread (or [`dispose`](Self::dispose) on a worker) instead.
    pub async fn dispose_async(&self) -> Result<()> {
        self.dispose()
    }
}

impl Debug for Fiber {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fiber")
            .field("uid", &self.uid())
            .field("name", &self.name())
            .field("state", &self.state())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::plugin_sync;
    use crate::{Inject, PluginOutput};
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    /// `dispose_async` is a synchronous pass-through: awaited from an async
    /// context it completes the whole disposal without deadlocking or
    /// needing an executor to make progress.
    #[test]
    fn dispose_async_completes_from_an_async_context() {
        let root = Context::new();
        let disposed = Arc::new(AtomicUsize::new(0));
        let disposed_in_disposer = disposed.clone();
        let fiber = root.plugin_default(plugin_sync::<(), _>(
            "teardown",
            Inject::default(),
            move |_, _| {
                let disposed_in_disposer = disposed_in_disposer.clone();
                Ok(PluginOutput::infallible(move || {
                    disposed_in_disposer.fetch_add(1, Ordering::SeqCst);
                }))
            },
        ));
        fiber.try_wait().unwrap();
        // A minimal waker-less block_on: the future never actually pending
        // is exactly what this test pins down.
        block_on(fiber.dispose_async()).unwrap();
        assert_eq!(fiber.state(), FiberState::Disposed);
        assert_eq!(disposed.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn idle_reflects_and_probes_transitions() {
        let root = Context::new();
        let observed = Arc::new(std::sync::Mutex::new(Vec::<bool>::new()));
        let fiber = root.plugin_default(plugin_sync::<(), _>("probe", Inject::default(), {
            let observed = observed.clone();
            move |ctx, _| {
                // apply() runs inside the fiber's own transition: the probe
                // must report busy where await_idle would deadlock.
                lock(&observed).push(ctx.fiber()?.idle());
                Ok(PluginOutput::none())
            }
        }));
        fiber.try_wait().unwrap();
        assert_eq!(*lock(&observed), vec![false]);
        assert!(fiber.idle());
    }

    /// Regression: a notification landing between refresh()'s final
    /// `dirty.swap(false)` and the transition lock release used to be lost —
    /// the notifier's try_lock failed and the holder had already stopped
    /// draining, leaving the fiber stale (and `dirty` set) until the next
    /// unrelated event. The armed stretch hook makes that window wide
    /// enough for the main thread to land a notification inside it.
    #[test]
    fn notify_between_final_check_and_unlock_is_not_lost() {
        let root = Context::new();
        let starts = Arc::new(AtomicUsize::new(0));
        let fiber = root.plugin_default(plugin_sync::<(), _>("consumer", Inject::new(["svc"]), {
            let starts = starts.clone();
            move |_, _| {
                starts.fetch_add(1, Ordering::SeqCst);
                Ok(PluginOutput::none())
            }
        }));
        let provider = root.provide("svc", 1_u32).unwrap();
        fiber.try_wait().unwrap();
        assert_eq!(starts.load(Ordering::SeqCst), 1);

        REFRESH_WINDOW_STRETCH.store(true, Ordering::Relaxed);
        // The disposal notifies the consumer and drives refresh() on this
        // thread; the armed hook parks it inside the window while still
        // holding the transition lock.
        let unloader = std::thread::spawn(move || provider.dispose());
        // Wait until the unload is visible: the worker is then inside the
        // stretched window, still holding the lock.
        let deadline = Instant::now() + Duration::from_secs(5);
        while fiber.state() != FiberState::Pending {
            assert!(Instant::now() < deadline, "unload did not start");
            std::thread::yield_now();
        }
        // Land a notification inside the window: this store(dirty) +
        // try_lock(fail) + return is exactly the previously lost wakeup.
        let _provider = root.provide("svc", 2_u32).unwrap();
        unloader.join().unwrap().unwrap();
        REFRESH_WINDOW_STRETCH.store(false, Ordering::Relaxed);

        // The re-provide must have been consumed without any further event.
        assert!(!fiber.inner.dirty.load(Ordering::Acquire));
        assert_eq!(fiber.state(), FiberState::Active);
        assert_eq!(starts.load(Ordering::SeqCst), 2);
    }

    /// Regression (issue #25): a hung `apply` keeps the transition mutex
    /// locked forever; `dispose` from another thread must give up with an
    /// error naming the fiber instead of blocking indefinitely.
    #[test]
    fn dispose_times_out_when_apply_hangs() {
        let root = Context::new();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        // mpsc receivers are not Sync; the mutex makes the callback shareable
        // while recv() parks the apply under it.
        let rx = Mutex::new(rx);
        let fiber = root.plugin_default(plugin_sync::<(), _>(
            "hung",
            Inject::new(["svc"]),
            move |_, _| {
                // Park until the sender drops: apply never finishes on its
                // own.
                let _ = lock(&rx).recv();
                Ok(PluginOutput::none())
            },
        ));
        assert_eq!(fiber.state(), FiberState::Pending);

        // Providing the dependency drives refresh() — and the parked
        // apply() — on the provider thread, which then holds the transition
        // mutex for as long as apply parks.
        let provider = std::thread::spawn({
            let root = root.clone();
            move || root.provide("svc", 1_u32)
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while fiber.state() != FiberState::Loading {
            assert!(Instant::now() < deadline, "apply never started");
            std::thread::yield_now();
        }

        TRANSITION_WAIT_MILLIS.store(150, Ordering::Relaxed);
        let started = Instant::now();
        let error = fiber.dispose().expect_err("dispose must time out");
        TRANSITION_WAIT_MILLIS.store(0, Ordering::Relaxed);

        assert!(
            started.elapsed() < Duration::from_secs(5),
            "dispose blocked far beyond the wait bound"
        );
        let message = error.to_string();
        assert!(message.contains("timed out"), "{message}");
        assert!(message.contains("hung"), "{message}");
        assert!(
            message.contains(&fiber.uid().unwrap().to_string()),
            "{message}"
        );

        // Let the parked apply finish so the helper thread can join.
        drop(tx);
        provider.join().unwrap().unwrap();
    }

    /// Regression (issue #25): two fibers whose disposers dispose each
    /// other deadlocked AB-BA on the transition mutexes. The bounded wait
    /// turns the standoff into two timeout errors naming the peers.
    #[test]
    fn cross_fiber_dispose_from_disposers_times_out_instead_of_deadlocking() {
        // Register an effect whose disposer disposes the fiber parked in
        // `target_slot`, but only after rendezvousing with the peer
        // disposer: the barrier guarantees both transition mutexes are held
        // when each disposer starts waiting for the other's.
        fn cross_disposer(
            ctx: &Context,
            target_slot: &Arc<Mutex<Option<Fiber>>>,
            rendezvous: &Arc<std::sync::Barrier>,
            sink: &Arc<Mutex<Vec<String>>>,
        ) -> Result<PluginOutput> {
            let target_slot = target_slot.clone();
            let rendezvous = rendezvous.clone();
            let sink = sink.clone();
            ctx.effect("cross", move || {
                let other = lock(&target_slot).clone();
                let Some(other) = other else {
                    return Ok(());
                };
                rendezvous.wait();
                match other.dispose() {
                    Ok(()) => lock(&sink).push("disposed".to_owned()),
                    Err(error) => {
                        lock(&sink).push(error.to_string());
                        // Keep this fiber's transition held a while longer so
                        // the peer's wait also expires: otherwise this side's
                        // teardown can release the lock inside the peer's
                        // deadline and the loser no-ops with Ok instead.
                        std::thread::sleep(Duration::from_millis(300));
                    }
                }
                Ok(())
            })?;
            Ok(PluginOutput::none())
        }

        let root = Context::new();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let outcomes = Arc::new(Mutex::new(Vec::<String>::new()));
        let target_of_a = Arc::new(Mutex::new(None::<Fiber>));
        let target_of_b = Arc::new(Mutex::new(None::<Fiber>));

        let a = root.plugin_default(plugin_sync::<(), _>("peer-a", Inject::none(), {
            let (target, rendezvous, sink) =
                (target_of_b.clone(), barrier.clone(), outcomes.clone());
            move |ctx, _| cross_disposer(&ctx, &target, &rendezvous, &sink)
        }));
        let b = root.plugin_default(plugin_sync::<(), _>("peer-b", Inject::none(), {
            let (target, rendezvous, sink) =
                (target_of_a.clone(), barrier.clone(), outcomes.clone());
            move |ctx, _| cross_disposer(&ctx, &target, &rendezvous, &sink)
        }));
        *lock(&target_of_a) = Some(a.clone());
        *lock(&target_of_b) = Some(b.clone());
        assert_eq!(a.state(), FiberState::Active);
        assert_eq!(b.state(), FiberState::Active);

        TRANSITION_WAIT_MILLIS.store(150, Ordering::Relaxed);
        let (tx_a, rx_a) = std::sync::mpsc::channel();
        let (tx_b, rx_b) = std::sync::mpsc::channel();
        let disposer_a = std::thread::spawn({
            let a = a.clone();
            move || {
                let _ = tx_a.send(a.dispose());
            }
        });
        let disposer_b = std::thread::spawn({
            let b = b.clone();
            move || {
                let _ = tx_b.send(b.dispose());
            }
        });
        for rx in [rx_a, rx_b] {
            // The outer dispose results are irrelevant here: completing
            // within the bound — not their values — is the regression check.
            let _ = rx
                .recv_timeout(Duration::from_secs(5))
                .expect("deadlock: cross-fiber dispose never returned");
        }
        TRANSITION_WAIT_MILLIS.store(0, Ordering::Relaxed);
        disposer_a.join().unwrap();
        disposer_b.join().unwrap();

        let messages = lock(&outcomes).clone();
        assert_eq!(messages.len(), 2, "{messages:?}");
        assert!(
            messages.iter().all(|message| message.contains("timed out")),
            "{messages:?}"
        );
        assert!(
            messages.iter().any(|message| message.contains("peer-a")),
            "{messages:?}"
        );
        assert!(
            messages.iter().any(|message| message.contains("peer-b")),
            "{messages:?}"
        );
    }
}
