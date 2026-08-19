//! Root and child contexts tying all Cordis services together.

use crate::effect::{AsyncDisposer, EffectHandle};
use crate::events::{Event, EventOptions, EventResult, EventValue, EventsRoot, EventsService};
use crate::fiber::{Fiber, FiberInner};
use crate::logger::{LogArg, Logger, LoggerRoot, LoggerService};
use crate::reflect::{Accessor, ReflectRoot, ReflectService};
use crate::registry::{Inject, IntoPlugin, PluginOutput, RegistryRoot, RegistryService};
use crate::{Config, Result, Value};
use std::collections::HashMap;
use std::fmt::{self, Debug, Formatter};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Weak};

/// Opaque service-scope label used by [`Context::isolate_with`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Isolation(pub(crate) u64);

impl Isolation {
    /// Create a user-defined label.
    ///
    /// Labels returned by [`Context::new_isolation`] are preferred because
    /// they cannot accidentally collide with framework-generated labels.
    pub const fn from_raw(value: u64) -> Self {
        Self(value)
    }

    /// Expose the numeric label for persistence or diagnostics.
    pub const fn as_raw(self) -> u64 {
        self.0
    }
}

/// Event listener filter attached to a child context.
pub type ContextFilter = Arc<dyn Fn(&Context) -> bool + Send + Sync + 'static>;

/// Immutable data inherited by contexts and copied on extension.
#[derive(Clone, Default)]
pub struct ContextMeta {
    pub(crate) isolates: Arc<HashMap<String, Isolation>>,
    pub(crate) intercepts: Arc<Vec<(String, Value)>>,
    pub(crate) values: Arc<HashMap<String, Value>>,
    pub(crate) filter: Option<ContextFilter>,
    pub(crate) base_url: Option<Arc<str>>,
}

impl Debug for ContextMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContextMeta")
            .field("isolates", &self.isolates)
            .field("intercept_count", &self.intercepts.len())
            .field("value_keys", &self.values.keys().collect::<Vec<_>>())
            .field("has_filter", &self.filter.is_some())
            .field("base_url", &self.base_url)
            .finish()
    }
}

pub(crate) struct RootInner {
    pub(crate) reflect: ReflectRoot,
    pub(crate) registry: RegistryRoot,
    pub(crate) events: EventsRoot,
    pub(crate) logger: LoggerRoot,
    pub(crate) next_scope: AtomicU64,
    pub(crate) next_fiber: AtomicU64,
    pub(crate) next_effect: AtomicU64,
    pub(crate) root_fiber: OnceLock<Fiber>,
}

impl RootInner {
    fn new() -> Self {
        Self {
            reflect: ReflectRoot::new(),
            registry: RegistryRoot::new(),
            events: EventsRoot::new(),
            logger: LoggerRoot::new(),
            // Keep 0 reserved for an absent scope/fiber/effect.
            next_scope: AtomicU64::new(0),
            next_fiber: AtomicU64::new(0),
            next_effect: AtomicU64::new(0),
            root_fiber: OnceLock::new(),
        }
    }

    pub(crate) fn scope(&self) -> Isolation {
        Isolation(self.next_scope.fetch_add(1, Ordering::Relaxed) + 1)
    }

    pub(crate) fn fiber_id(&self) -> u64 {
        self.next_fiber.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub(crate) fn effect_id(&self) -> u64 {
        self.next_effect.fetch_add(1, Ordering::Relaxed) + 1
    }
}

/// Root and child dependency containers for Cordis plugins.
///
/// Cloning a context is cheap. Child contexts share all runtime services while
/// carrying immutable scope/intercept/metadata overlays and the current fiber.
#[derive(Clone)]
pub struct Context {
    pub(crate) root: Arc<RootInner>,
    pub(crate) fiber: Weak<FiberInner>,
    pub(crate) meta: ContextMeta,
}

impl Context {
    /// Create a root context and install the core reflection, registry, event,
    /// and logger services.
    pub fn new() -> Self {
        let root = Arc::new(RootInner::new());
        let fiber = Fiber::new_root(Arc::downgrade(&root), ContextMeta::default());
        root.root_fiber
            .set(fiber.clone())
            .unwrap_or_else(|_| unreachable!("new root has no fiber"));
        fiber.context().expect("fresh root is alive")
    }

    /// Return whether two contexts belong to the same root application.
    pub fn same_root(&self, other: &Context) -> bool {
        Arc::ptr_eq(&self.root, &other.root)
    }

    /// Return the root context.
    pub fn root(&self) -> Context {
        self.root
            .root_fiber
            .get()
            .expect("root fiber initialized")
            .context()
            .expect("root context has a live root")
    }

    /// Return this context's owning plugin fiber.
    pub fn fiber(&self) -> Result<Fiber> {
        self.fiber
            .upgrade()
            .map(Fiber::from_inner)
            .ok_or_else(|| crate::CordisError::new(crate::ErrorCode::InactiveEffect))
    }

    /// Return the base URL inherited by this context, when set.
    pub fn base_url(&self) -> Option<&str> {
        self.meta.base_url.as_deref()
    }

    /// Derive a child context with a new base URL.
    pub fn with_base_url(&self, base_url: impl Into<Arc<str>>) -> Context {
        let mut child = self.clone();
        child.meta.base_url = Some(base_url.into());
        child
    }

    /// Derive a child context carrying arbitrary metadata.
    pub fn extend<T>(&self, name: impl Into<String>, value: T) -> Context
    where
        T: Send + Sync + 'static,
    {
        self.extend_value(name, Value::new(value))
    }

    /// Derive a child context carrying type-erased metadata.
    pub fn extend_value(&self, name: impl Into<String>, value: Value) -> Context {
        let mut values = (*self.meta.values).clone();
        values.insert(name.into(), value);
        let mut child = self.clone();
        child.meta.values = Arc::new(values);
        child
    }

    /// Read typed context metadata.
    pub fn metadata<T>(&self, name: &str) -> Result<Option<Arc<T>>>
    where
        T: Send + Sync + 'static,
    {
        self.meta.values.get(name).map(Value::downcast).transpose()
    }

    /// Allocate a globally unique isolation label.
    pub fn new_isolation(&self) -> Isolation {
        self.root.scope()
    }

    /// Create a child context with a fresh independent service scope for
    /// `name`.
    pub fn isolate(&self, name: impl Into<String>) -> Context {
        let label = self.new_isolation();
        self.isolate_with(name, label)
    }

    /// Create a child context using a supplied scope label. Reusing the label
    /// joins otherwise separate context branches to the same service scope.
    ///
    /// A scope holds one implementation per slot, and every name isolated
    /// with the same label maps to that one slot: only one of those names
    /// can be provided there, and providing a second fails with
    /// [`DuplicateService`](crate::ErrorCode::DuplicateService).
    pub fn isolate_with(&self, name: impl Into<String>, label: Isolation) -> Context {
        let mut isolates = (*self.meta.isolates).clone();
        isolates.insert(name.into(), label);
        let mut child = self.clone();
        child.meta.isolates = Arc::new(isolates);
        child
    }

    /// Create a child context with several service names isolated together.
    ///
    /// All `names` share one scope label and therefore one implementation
    /// slot, matching the upstream single-slot scope model: provide exactly
    /// one of them in this branch — providing a second name fails with
    /// [`DuplicateService`](crate::ErrorCode::DuplicateService) — and inject
    /// only that name from sibling plugins. Use separate labels (or
    /// [`isolate`](Self::isolate)) when several of the names need independent
    /// implementations.
    pub fn isolate_many(
        &self,
        names: impl IntoIterator<Item = impl Into<String>>,
        label: Option<Isolation>,
    ) -> Context {
        let label = label.unwrap_or_else(|| self.new_isolation());
        let mut child = self.clone();
        let mut isolates = (*child.meta.isolates).clone();
        for name in names {
            isolates.insert(name.into(), label);
        }
        child.meta.isolates = Arc::new(isolates);
        child
    }

    /// Add service-specific intercept configuration below this context.
    pub fn intercept<T>(&self, name: impl Into<String>, config: T) -> Context
    where
        T: Send + Sync + 'static,
    {
        self.intercept_value(name, Value::new(config))
    }

    /// Add type-erased intercept configuration.
    pub fn intercept_value(&self, name: impl Into<String>, config: Value) -> Context {
        let mut intercepts = (*self.meta.intercepts).clone();
        intercepts.push((name.into(), config));
        let mut child = self.clone();
        child.meta.intercepts = Arc::new(intercepts);
        child
    }

    /// Return typed intercept configs in ancestor-to-descendant order.
    pub fn intercepts<T>(&self, name: &str) -> Result<Vec<Arc<T>>>
    where
        T: Send + Sync + 'static,
    {
        self.meta
            .intercepts
            .iter()
            .filter(|(entry, _)| entry == name)
            .map(|(_, value)| value.downcast())
            .collect()
    }

    /// Attach an event listener filter to a derived context.
    pub fn with_filter<F>(&self, filter: F) -> Context
    where
        F: Fn(&Context) -> bool + Send + Sync + 'static,
    {
        let mut child = self.clone();
        child.meta.filter = Some(Arc::new(filter));
        child
    }

    /// Return the events service bound to this context.
    pub fn events(&self) -> EventsService {
        EventsService::new(self.clone())
    }

    /// Return the reflection/service store bound to this context.
    pub fn reflect(&self) -> ReflectService {
        ReflectService::new(self.clone())
    }

    /// Return the plugin registry bound to this context.
    pub fn registry(&self) -> RegistryService {
        RegistryService::new(self.clone())
    }

    /// Return a logger named from the current fiber and intercepts.
    ///
    /// This is the upstream-semantics facade: a [`Logger`] to call
    /// `error`/`info`/`warn`/`debug` on. Managing the logging machinery —
    /// registering exporters, reading or resizing the buffer — goes through
    /// [`logger_service`](Self::logger_service) instead.
    pub fn logger(&self) -> Logger {
        LoggerService::new(self.clone()).logger(None)
    }

    /// Return an explicitly named logger.
    pub fn named_logger(&self, name: impl Into<String>) -> Logger {
        LoggerService::new(self.clone()).logger(Some(name.into()))
    }

    /// Return the logger service bound to this context.
    ///
    /// The service side of the logger split: exporter registration
    /// ([`exporter`](crate::LoggerService::exporter) and variants) plus
    /// buffer inspection and sizing. To emit messages, create a logger with
    /// [`logger`](Self::logger) or [`named_logger`](Self::named_logger)
    /// instead.
    pub fn logger_service(&self) -> LoggerService {
        LoggerService::new(self.clone())
    }

    /// Register a synchronous cleanup operation on the current fiber.
    pub fn effect<F>(&self, label: impl Into<String>, dispose: F) -> Result<EffectHandle>
    where
        F: FnOnce() -> Result<()> + Send + 'static,
    {
        self.fiber()?
            .register_effect(label, AsyncDisposer::from_sync(dispose))
    }

    /// Register an infallible synchronous cleanup operation.
    pub fn effect_infallible<F>(&self, label: impl Into<String>, dispose: F) -> Result<EffectHandle>
    where
        F: FnOnce() + Send + 'static,
    {
        self.fiber()?
            .register_effect(label, AsyncDisposer::infallible(dispose))
    }

    /// Register an asynchronous cleanup operation.
    pub fn effect_async<F, Fut>(&self, label: impl Into<String>, dispose: F) -> Result<EffectHandle>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.fiber()?
            .register_effect(label, AsyncDisposer::from_async(dispose))
    }

    /// Provide a concrete service in this context's isolation scope.
    pub fn provide<T>(&self, name: impl Into<String>, value: T) -> Result<EffectHandle>
    where
        T: Send + Sync + 'static,
    {
        self.reflect()
            .provide_value(name.into(), Value::new(value), None)
    }

    /// Provide an existing `Arc` without wrapping it in a second `Arc`.
    pub fn provide_arc<T>(&self, name: impl Into<String>, value: Arc<T>) -> Result<EffectHandle>
    where
        T: Send + Sync + 'static,
    {
        self.reflect()
            .provide_value(name.into(), Value::from_arc(value), None)
    }

    /// Read a service without enforcing an inject declaration.
    pub fn get<T>(&self, name: &str) -> Result<Option<Arc<T>>>
    where
        T: Send + Sync + 'static,
    {
        self.reflect().get(name)
    }

    /// Read a service even while its provider is loading or unloading.
    pub fn get_relaxed<T>(&self, name: &str) -> Result<Option<Arc<T>>>
    where
        T: Send + Sync + 'static,
    {
        self.reflect().get_relaxed(name)
    }

    /// Require a currently active service.
    ///
    /// Runtime resolution. The identically named
    /// [`Inject::require`](crate::Inject::require) is the declaration-time
    /// builder that records a dependency for startup instead of resolving
    /// one now.
    pub fn require<T>(&self, name: &str) -> Result<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        self.reflect().require(name)
    }

    /// Replace a service value. Only its providing fiber may do this.
    ///
    /// The replacement does not wake dependent fibers; see
    /// [`ReflectService::set_value`](crate::ReflectService::set_value) and
    /// [`notify`](Self::notify).
    pub fn set<T>(&self, name: &str, value: T) -> Result<()>
    where
        T: Send + Sync + 'static,
    {
        self.reflect().set_value(name, Value::new(value))
    }

    /// Re-evaluate dependency availability for the named services.
    pub fn notify<I, S>(&self, names: I) -> Vec<Fiber>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.reflect().notify(names)
    }

    /// Register a dynamic computed property.
    pub fn accessor(&self, name: impl Into<String>, accessor: Accessor) -> Result<EffectHandle> {
        self.reflect().accessor(name.into(), accessor)
    }

    /// Register an event listener owned by this context's fiber.
    pub fn on<F>(&self, name: impl Into<String>, listener: F) -> Result<EffectHandle>
    where
        F: Fn(Event) -> EventResult + Send + Sync + 'static,
    {
        self.events().on(name, listener, EventOptions::default())
    }

    /// Register a listener with placement and filtering options.
    pub fn on_with<F>(
        &self,
        name: impl Into<String>,
        listener: F,
        options: EventOptions,
    ) -> Result<EffectHandle>
    where
        F: Fn(Event) -> EventResult + Send + Sync + 'static,
    {
        self.events().on(name, listener, options)
    }

    /// Register an asynchronous event listener.
    ///
    /// See [`EventsService::on_async`](crate::EventsService::on_async) for the
    /// blocking-executor caveats before awaiting thread-local work here.
    pub fn on_async<F, Fut>(&self, name: impl Into<String>, listener: F) -> Result<EffectHandle>
    where
        F: Fn(Event) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = EventResult> + Send + 'static,
    {
        self.events()
            .on_async(name, listener, EventOptions::default())
    }

    /// Register an asynchronous listener with placement and filtering
    /// options.
    ///
    /// See [`EventsService::on_async`](crate::EventsService::on_async) for the
    /// blocking-executor caveats before awaiting thread-local work here.
    pub fn on_async_with<F, Fut>(
        &self,
        name: impl Into<String>,
        listener: F,
        options: EventOptions,
    ) -> Result<EffectHandle>
    where
        F: Fn(Event) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = EventResult> + Send + 'static,
    {
        self.events().on_async(name, listener, options)
    }

    /// Register a one-shot event listener.
    pub fn once<F>(&self, name: impl Into<String>, listener: F) -> Result<EffectHandle>
    where
        F: Fn(Event) -> EventResult + Send + Sync + 'static,
    {
        self.events().once(name, listener, EventOptions::default())
    }

    /// Emit an event synchronously.
    pub fn emit(
        &self,
        name: impl Into<String>,
        args: impl IntoIterator<Item = EventValue>,
    ) -> Result<()> {
        self.events().emit(name, args)
    }

    /// Run all matching listeners concurrently.
    pub async fn parallel(
        &self,
        name: impl Into<String>,
        args: impl IntoIterator<Item = EventValue>,
    ) -> Result<()> {
        self.events().parallel(name, args).await
    }

    /// Await listeners in order until one returns a bail value.
    pub async fn serial(
        &self,
        name: impl Into<String>,
        args: impl IntoIterator<Item = EventValue>,
    ) -> EventResult {
        self.events().serial(name, args).await
    }

    /// Run listeners synchronously until one returns a bail value.
    pub fn bail(
        &self,
        name: impl Into<String>,
        args: impl IntoIterator<Item = EventValue>,
    ) -> EventResult {
        self.events().bail(name, args)
    }

    /// Compose listeners around an innermost synchronous callback.
    pub fn waterfall<F>(
        &self,
        name: impl Into<String>,
        args: impl IntoIterator<Item = EventValue>,
        inner: F,
    ) -> EventResult
    where
        F: Fn() -> EventResult + Send + Sync + 'static,
    {
        self.events().waterfall(name, args, inner)
    }

    /// Compose listeners around an innermost asynchronous callback.
    pub async fn waterfall_async<F, Fut>(
        &self,
        name: impl Into<String>,
        args: impl IntoIterator<Item = EventValue>,
        inner: F,
    ) -> EventResult
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = EventResult> + Send + 'static,
    {
        self.events().waterfall_async(name, args, inner).await
    }

    /// Start a plugin and return its lifecycle fiber.
    pub fn plugin<P, C>(&self, plugin: P, config: C) -> Fiber
    where
        P: IntoPlugin,
        C: Send + Sync + 'static,
    {
        self.registry()
            .plugin_value(plugin.into_plugin(), Config::new(config))
    }

    /// Start a plugin with unit configuration.
    pub fn plugin_default<P>(&self, plugin: P) -> Fiber
    where
        P: IntoPlugin,
    {
        self.registry()
            .plugin_value(plugin.into_plugin(), Config::default())
    }

    /// Startup shortcut: run `callback` once every service in `inject` is
    /// available, wrapping it in an anonymous plugin fiber.
    ///
    /// Not to be confused with [`Fiber::inject`](crate::Fiber::inject),
    /// which reads back the normalized declaration of an existing fiber.
    pub fn inject<F>(&self, inject: Inject, callback: F) -> Fiber
    where
        F: Fn(Context) -> Result<PluginOutput> + Send + Sync + 'static,
    {
        let plugin = crate::plugin_sync::<(), _>("anonymous", inject, move |ctx, _| callback(ctx));
        self.plugin_default(plugin)
    }

    /// Log an error through the current context logger.
    pub fn log_error(&self, error: impl ToString) {
        self.logger().error(error.to_string(), Vec::<LogArg>::new());
    }

    pub(crate) fn scope_override(&self, name: &str) -> Option<Isolation> {
        self.meta.isolates.get(name).copied()
    }

    pub(crate) fn filter(&self) -> Option<&ContextFilter> {
        self.meta.filter.as_ref()
    }

    pub(crate) fn root_arc(&self) -> &Arc<RootInner> {
        &self.root
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

impl Debug for Context {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let name = self
            .fiber()
            .map(|fiber| fiber.name())
            .unwrap_or_else(|_| "disposed".to_owned());
        f.debug_tuple("Context").field(&name).finish()
    }
}
