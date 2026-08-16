//! Plugin entrypoints, dependency declarations, and runtime registry.

use crate::context::Context;
use crate::effect::AsyncDisposer;
use crate::fiber::{Fiber, FiberInner};
use crate::utils::{BoxFuture, lock};
use crate::{Config, CordisError, ErrorCode, Result, Value};
use std::fmt::{self, Debug, Formatter};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

/// One required service and its optional intercept configuration.
#[derive(Debug, Clone)]
pub struct Dependency {
    /// Required service name.
    pub name: String,
    /// Config appended to that service's intercept chain in the plugin context.
    pub config: Option<Value>,
}

/// Normalized plugin service dependencies.
#[derive(Debug, Clone, Default)]
pub struct Inject {
    entries: Vec<Dependency>,
}

impl Inject {
    /// Construct an inject declaration from service names.
    pub fn new<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            entries: names
                .into_iter()
                .map(|name| Dependency {
                    name: name.into(),
                    config: None,
                })
                .collect(),
        }
    }

    /// Construct an empty declaration.
    pub fn none() -> Self {
        Self::default()
    }

    /// Add a required service without intercept config.
    pub fn require(mut self, name: impl Into<String>) -> Self {
        self.entries.push(Dependency {
            name: name.into(),
            config: None,
        });
        self
    }

    /// Add a required service and intercept config.
    pub fn require_with<T>(mut self, name: impl Into<String>, config: T) -> Self
    where
        T: Send + Sync + 'static,
    {
        self.entries.push(Dependency {
            name: name.into(),
            config: Some(Value::new(config)),
        });
        self
    }

    /// Add a type-erased intercept config.
    pub fn require_with_value(mut self, name: impl Into<String>, config: Value) -> Self {
        self.entries.push(Dependency {
            name: name.into(),
            config: Some(config),
        });
        self
    }

    /// Iterate dependencies in declaration order.
    pub fn iter(&self) -> impl Iterator<Item = &Dependency> {
        self.entries.iter()
    }

    /// Number of dependencies.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no services are required.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether this declaration contains `name`.
    pub fn contains(&self, name: &str) -> bool {
        self.entries.iter().any(|entry| entry.name == name)
    }

    /// Return just the service names.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|entry| entry.name.as_str())
    }
}

impl<const N: usize> From<[&str; N]> for Inject {
    fn from(value: [&str; N]) -> Self {
        Self::new(value)
    }
}

impl From<Vec<String>> for Inject {
    fn from(value: Vec<String>) -> Self {
        Self::new(value)
    }
}

/// Resources returned by plugin startup and owned by its fiber.
#[derive(Debug, Default)]
pub struct PluginOutput {
    pub(crate) disposers: Vec<(String, AsyncDisposer)>,
}

impl PluginOutput {
    /// Return no additional cleanup. Effects registered through `ctx` are
    /// still owned by the plugin fiber.
    pub fn none() -> Self {
        Self::default()
    }

    /// Return one synchronous cleanup operation.
    pub fn disposer<F>(dispose: F) -> Self
    where
        F: FnOnce() -> Result<()> + Send + 'static,
    {
        Self::default().with_disposer("plugin return", dispose)
    }

    /// Return one infallible synchronous cleanup operation.
    pub fn infallible<F>(dispose: F) -> Self
    where
        F: FnOnce() + Send + 'static,
    {
        let mut output = Self::default();
        output.disposers.push((
            "plugin return".to_owned(),
            AsyncDisposer::infallible(dispose),
        ));
        output
    }

    /// Append a named synchronous cleanup operation.
    pub fn with_disposer<F>(mut self, label: impl Into<String>, dispose: F) -> Self
    where
        F: FnOnce() -> Result<()> + Send + 'static,
    {
        self.disposers
            .push((label.into(), AsyncDisposer::from_sync(dispose)));
        self
    }

    /// Append an already boxed asynchronous disposer.
    pub fn with_async_disposer(mut self, label: impl Into<String>, dispose: AsyncDisposer) -> Self {
        self.disposers.push((label.into(), dispose));
        self
    }
}

/// Object-safe Cordis plugin entrypoint.
///
/// Implementations receive type-erased config to support heterogeneous plugin
/// registries. [`plugin_sync`] and [`plugin_async`] provide typed adapters for
/// ordinary closures.
pub trait Plugin: Send + Sync + 'static {
    /// Display name used by fibers and loggers.
    fn name(&self) -> &str;

    /// Required services. Startup waits in `Pending` until all are active.
    fn inject(&self) -> Inject {
        Inject::default()
    }

    /// Validate and optionally normalize raw config before startup.
    fn validate_config(&self, config: Config) -> Result<Config> {
        Ok(config)
    }

    /// Start this plugin in `ctx`.
    fn apply(&self, ctx: Context, config: Config) -> BoxFuture<Result<PluginOutput>>;
}

/// Stable identity shared by every fiber started from one [`PluginHandle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PluginKey(pub u64);

static NEXT_PLUGIN: AtomicU64 = AtomicU64::new(0);

/// Cloneable, dynamically dispatched plugin with stable registry identity.
#[derive(Clone)]
pub struct PluginHandle {
    key: PluginKey,
    plugin: Arc<dyn Plugin>,
}

impl PluginHandle {
    /// Wrap a plugin implementation.
    pub fn new<P: Plugin>(plugin: P) -> Self {
        Self {
            key: PluginKey(NEXT_PLUGIN.fetch_add(1, Ordering::Relaxed) + 1),
            plugin: Arc::new(plugin),
        }
    }

    /// Return this callback's stable identity.
    pub const fn key(&self) -> PluginKey {
        self.key
    }

    /// Return the plugin display name.
    pub fn name(&self) -> &str {
        self.plugin.name()
    }

    pub(crate) fn plugin(&self) -> &Arc<dyn Plugin> {
        &self.plugin
    }
}

impl Debug for PluginHandle {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("PluginHandle")
            .field("key", &self.key)
            .field("name", &self.name())
            .finish()
    }
}

/// Conversion accepted by [`Context::plugin`](crate::Context::plugin).
pub trait IntoPlugin {
    /// Produce a stable plugin handle.
    fn into_plugin(self) -> PluginHandle;
}

impl IntoPlugin for PluginHandle {
    fn into_plugin(self) -> PluginHandle {
        self
    }
}

struct FunctionPlugin {
    name: String,
    inject: Inject,
    callback: Arc<dyn Fn(Context, Config) -> BoxFuture<Result<PluginOutput>> + Send + Sync>,
}

impl Plugin for FunctionPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn inject(&self) -> Inject {
        self.inject.clone()
    }

    fn apply(&self, ctx: Context, config: Config) -> BoxFuture<Result<PluginOutput>> {
        (self.callback)(ctx, config)
    }
}

/// Adapt a typed synchronous closure to a Cordis plugin.
pub fn plugin_sync<C, F>(name: impl Into<String>, inject: Inject, callback: F) -> PluginHandle
where
    C: Send + Sync + 'static,
    F: Fn(Context, Arc<C>) -> Result<PluginOutput> + Send + Sync + 'static,
{
    let callback = Arc::new(callback);
    PluginHandle::new(FunctionPlugin {
        name: name.into(),
        inject,
        callback: Arc::new(move |ctx, config| {
            let callback = callback.clone();
            Box::pin(async move {
                let config = config.downcast::<C>().map_err(|error| {
                    CordisError::with_message(
                        ErrorCode::InvalidConfig,
                        format!("invalid config type: {error}"),
                    )
                })?;
                callback(ctx, config)
            })
        }),
    })
}

/// Adapt a typed asynchronous closure to a Cordis plugin.
pub fn plugin_async<C, F, Fut>(name: impl Into<String>, inject: Inject, callback: F) -> PluginHandle
where
    C: Send + Sync + 'static,
    F: Fn(Context, Arc<C>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<PluginOutput>> + Send + 'static,
{
    let callback = Arc::new(callback);
    PluginHandle::new(FunctionPlugin {
        name: name.into(),
        inject,
        callback: Arc::new(move |ctx, config| {
            let callback = callback.clone();
            Box::pin(async move {
                let config = config.downcast::<C>().map_err(|error| {
                    CordisError::with_message(
                        ErrorCode::InvalidConfig,
                        format!("invalid config type: {error}"),
                    )
                })?;
                callback(ctx, config).await
            })
        }),
    })
}

pub(crate) struct RuntimeRecord {
    pub(crate) handle: PluginHandle,
    pub(crate) fibers: Vec<Weak<FiberInner>>,
}

#[derive(Default)]
pub(crate) struct RegistryState {
    pub(crate) runtimes: std::collections::BTreeMap<PluginKey, RuntimeRecord>,
    /// Service name → fibers injecting it, so service notifications scan only
    /// interested fibers instead of upgrading the whole registry.
    pub(crate) injectors: std::collections::HashMap<String, Vec<Weak<FiberInner>>>,
}

pub(crate) struct RegistryRoot {
    pub(crate) state: Mutex<RegistryState>,
}

impl RegistryRoot {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(RegistryState::default()),
        }
    }

    pub(crate) fn remove_fiber(&self, key: PluginKey, uid: u64) {
        let mut state = lock(&self.state);
        let remove_runtime = if let Some(runtime) = state.runtimes.get_mut(&key) {
            runtime.fibers.retain(|weak| {
                weak.upgrade()
                    .and_then(|fiber| fiber.uid_value())
                    .map(|fiber_uid| fiber_uid != uid)
                    .unwrap_or(false)
            });
            runtime.fibers.is_empty()
        } else {
            false
        };
        if remove_runtime {
            state.runtimes.remove(&key);
        }
        // The uid is cleared before removal, so the target matches via the
        // same None-collapse as above; dead weaks are pruned along the way.
        for weaks in state.injectors.values_mut() {
            weaks.retain(|weak| {
                weak.upgrade()
                    .and_then(|fiber| fiber.uid_value())
                    .map(|fiber_uid| fiber_uid != uid)
                    .unwrap_or(false)
            });
        }
        state.injectors.retain(|_, weaks| !weaks.is_empty());
    }

    /// Live fibers injecting `name`; prunes dead weak references as a side
    /// effect.
    pub(crate) fn fibers_injecting(&self, name: &str) -> Vec<Fiber> {
        let mut state = lock(&self.state);
        let Some(weaks) = state.injectors.get_mut(name) else {
            return Vec::new();
        };
        let mut fibers = Vec::with_capacity(weaks.len());
        weaks.retain(|weak| {
            if let Some(fiber) = weak.upgrade() {
                fibers.push(Fiber::from_inner(fiber));
                true
            } else {
                false
            }
        });
        if weaks.is_empty() {
            state.injectors.remove(name);
        }
        fibers
    }
}

/// Read-only snapshot of one plugin runtime.
#[derive(Debug, Clone)]
pub struct RuntimeInfo {
    /// Stable plugin identity.
    pub key: PluginKey,
    /// Display name.
    pub name: String,
    /// Live fibers for this callback.
    pub fibers: Vec<Fiber>,
}

/// Plugin registry bound to a context.
#[derive(Clone, Debug)]
pub struct RegistryService {
    ctx: Context,
}

impl RegistryService {
    pub(crate) fn new(ctx: Context) -> Self {
        Self { ctx }
    }

    /// Number of registered plugin callbacks.
    pub fn len(&self) -> usize {
        lock(&self.ctx.root.registry.state).runtimes.len()
    }

    /// Whether no plugins are registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether a plugin handle has at least one live fiber.
    pub fn contains(&self, plugin: &PluginHandle) -> bool {
        lock(&self.ctx.root.registry.state)
            .runtimes
            .contains_key(&plugin.key())
    }

    /// Return runtime snapshots.
    pub fn values(&self) -> Vec<RuntimeInfo> {
        let mut state = lock(&self.ctx.root.registry.state);
        state
            .runtimes
            .iter_mut()
            .map(|(key, runtime)| {
                let mut fibers = Vec::new();
                runtime.fibers.retain(|fiber| {
                    if let Some(fiber) = fiber.upgrade() {
                        fibers.push(Fiber::from_inner(fiber));
                        true
                    } else {
                        false
                    }
                });
                RuntimeInfo {
                    key: *key,
                    name: runtime.handle.name().to_owned(),
                    fibers,
                }
            })
            .collect()
    }

    /// Start a plugin from type-erased config.
    pub fn plugin_value(&self, plugin: PluginHandle, config: Config) -> Fiber {
        let fiber = Fiber::new_plugin(&self.ctx, plugin.clone(), config);
        let weak = Arc::downgrade(&fiber.inner);
        {
            let mut state = lock(&self.ctx.root.registry.state);
            state
                .runtimes
                .entry(plugin.key())
                .or_insert_with(|| RuntimeRecord {
                    handle: plugin.clone(),
                    fibers: Vec::new(),
                })
                .fibers
                .push(weak.clone());
            for dependency in fiber.inject().iter() {
                state
                    .injectors
                    .entry(dependency.name.clone())
                    .or_default()
                    .push(weak.clone());
            }
        }

        // Parent ownership mirrors the `ctx.plugin()` structural effect in
        // TypeScript. The registry itself only keeps weak fiber references.
        let owned = fiber.clone();
        match self.ctx.fiber().and_then(|parent| {
            parent.register_effect(
                "ctx.plugin()",
                AsyncDisposer::from_async(move || async move { owned.dispose_async().await }),
            )
        }) {
            Ok(effect) => fiber.set_parent_effect(effect),
            Err(error) => fiber.reject(error),
        }

        if fiber.uid().is_some() {
            let _ = self
                .ctx
                .events()
                .emit("internal/plugin", [Value::new(fiber.clone())]);
            fiber.refresh();
        }
        fiber
    }

    /// Dispose all fibers created from `plugin` and remove its runtime.
    pub fn delete(&self, plugin: &PluginHandle) -> bool {
        let fibers = {
            let mut state = lock(&self.ctx.root.registry.state);
            let Some(runtime) = state.runtimes.remove(&plugin.key()) else {
                return false;
            };
            runtime
                .fibers
                .into_iter()
                .filter_map(|fiber| fiber.upgrade())
                .map(Fiber::from_inner)
                .collect::<Vec<_>>()
        };
        for fiber in fibers {
            if let Err(error) = fiber.dispose() {
                self.ctx.log_error(error);
            }
        }
        true
    }
}
