//! Plugin fiber lifecycle, dependency epochs, and effect cleanup.

use crate::context::{Context, ContextMeta, Isolation, RootInner};
use crate::effect::{AsyncDisposer, EffectCell, EffectHandle, EffectMeta};
use crate::registry::{Inject, PluginHandle, PluginKey};
use crate::utils::{block_on, lock};
use crate::{Config, CordisError, ErrorCode, Result, Value};
use std::fmt::{self, Debug, Formatter};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

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

/// Cloneable handle to one plugin lifecycle instance.
#[derive(Clone)]
pub struct Fiber {
    pub(crate) inner: Arc<FiberInner>,
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
    pub fn context(&self) -> Context {
        Context {
            root: self.inner.root.upgrade().expect("live fiber has a root"),
            fiber: Arc::downgrade(&self.inner),
            meta: self.inner.meta.clone(),
        }
    }

    /// [`context`](Self::context) that tolerates a dropped root.
    ///
    /// Internal notification and logging paths skip their work when every
    /// `Context` is gone and only this fiber handle remains, rather than
    /// panicking.
    fn try_context(&self) -> Option<Context> {
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
        lock(&self.inner.effects).push(cell.clone());
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
        if let Some(ctx) = self.try_context() {
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
                if let Some(ctx) = self.try_context() {
                    ctx.log_error(error);
                }
            }
        }
    }

    fn dependency_epoch(&self) -> Option<Vec<u64>> {
        let root = self.inner.root.upgrade()?;
        root.dependency_epoch(&self.context(), &self.inner.inject)
    }

    fn activate(&self, epoch: Vec<u64>) {
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
            let output = block_on(plugin.plugin().apply(self.context(), config.clone()))?;
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
                if let Some(ctx) = self.try_context() {
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
        self.inner.dirty.store(true, Ordering::Release);
        let guard = match self.inner.transition.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return,
        };
        while self.inner.dirty.swap(false, Ordering::AcqRel) {
            self.reconcile();
        }
        drop(guard);
    }

    /// Lock the transition mutex, failing instead of deadlocking when the
    /// caller is already inside a transition on this fiber — directly or
    /// through a lifecycle callback such as an `internal/status` listener or
    /// a disposer. [`refresh`](Self::refresh) already degrades to a dirty flag
    /// in that situation; `restart`/`dispose` have no deferrable semantics,
    /// so they report the reentrancy as an error.
    fn try_transition(&self) -> Result<std::sync::MutexGuard<'_, ()>> {
        match self.inner.transition.try_lock() {
            Ok(guard) => Ok(guard),
            Err(std::sync::TryLockError::Poisoned(error)) => Ok(error.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => Err(CordisError::with_message(
                ErrorCode::Other,
                "reentrant lifecycle transition on the same fiber",
            )),
        }
    }

    /// Wait for the current lifecycle transition and rethrow startup errors.
    ///
    /// This port drives transitions eagerly, so this method does not require a
    /// particular async runtime.
    pub fn wait(&self) -> Result<Fiber> {
        if let Some(error) = self.error() {
            Err(error)
        } else {
            Ok(self.clone())
        }
    }

    /// Async equivalent of [`wait`](Self::wait).
    pub async fn await_ready(&self) -> Result<Fiber> {
        self.wait()
    }

    /// Dispose and immediately reload this plugin with its current config.
    pub fn restart(&self) -> Result<()> {
        self.assert_active()?;
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
        self.wait().map(|_| ())
    }

    /// Validate and apply new config, then restart when dependencies are active.
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
            self.refresh();
            self.wait().map(|_| ())
        }
    }

    /// Permanently dispose this plugin fiber. Repeated calls are no-ops.
    pub fn dispose(&self) -> Result<()> {
        // Take the transition lock up front: reentrant calls from lifecycle
        // callbacks fail fast instead of deadlocking, and no partial teardown
        // is left behind.
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

        if let Some(ctx) = self.try_context() {
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
