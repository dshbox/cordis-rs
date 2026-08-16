//! Disposal-aware event bus and Cordis dispatch strategies.

use crate::context::{Context, ContextMeta, RootInner};
use crate::effect::{AsyncDisposer, EffectHandle};
use crate::fiber::FiberInner;
use crate::utils::{BoxFuture, block_on, join_all, lock};
use crate::{CordisError, ErrorCode, Result, Value};
use std::collections::HashMap;
use std::fmt::{self, Debug, Formatter};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

/// Event argument and listener bail value.
pub type EventValue = Value;
/// Listener result. `Some(value)` bails; `None` continues dispatch.
pub type EventResult = Result<Option<EventValue>>;

/// Return whether a Rust event result should stop bail-style dispatch.
///
/// Rust uses `Option` instead of JavaScript truthiness: every `Some` value
/// bails, while `None` continues.
pub const fn is_bailed(value: &Option<EventValue>) -> bool {
    value.is_some()
}

/// Event dispatch strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DispatchMode {
    /// Synchronous observation.
    Emit,
    /// Concurrent futures.
    Parallel,
    /// Ordered asynchronous bail.
    Serial,
    /// Ordered synchronous bail.
    Bail,
    /// Middleware continuation chain.
    Waterfall,
}

/// Registered listener options.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EventOptions {
    /// Insert before existing listeners.
    pub prepend: bool,
    /// Ignore dispatch-context filter checks.
    pub global: bool,
}

/// Continuation supplied during waterfall dispatch.
pub type Next = Arc<dyn Fn() -> BoxFuture<EventResult> + Send + Sync + 'static>;

/// Owned event delivered to callbacks.
#[derive(Clone)]
pub struct Event {
    name: Arc<str>,
    args: Arc<[EventValue]>,
    target: Option<Context>,
    next: Option<Next>,
}

impl Event {
    /// Construct an event without a dispatch target.
    pub fn new(name: impl Into<String>, args: impl IntoIterator<Item = EventValue>) -> Self {
        Self {
            name: Arc::from(name.into()),
            args: args.into_iter().collect::<Vec<_>>().into(),
            target: None,
            next: None,
        }
    }

    /// Event name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Type-erased event arguments.
    pub fn args(&self) -> &[EventValue] {
        &self.args
    }

    /// Downcast one argument, returning `None` when out of bounds.
    pub fn arg<T>(&self, index: usize) -> Result<Option<Arc<T>>>
    where
        T: Send + Sync + 'static,
    {
        self.args.get(index).map(Value::downcast).transpose()
    }

    /// Explicit dispatch target used for context filtering.
    pub fn target(&self) -> Option<&Context> {
        self.target.as_ref()
    }

    /// Whether this callback is part of a waterfall chain.
    pub fn has_next(&self) -> bool {
        self.next.is_some()
    }

    /// Invoke the next waterfall listener or innermost behavior.
    pub fn next(&self) -> BoxFuture<EventResult> {
        match self.next.clone() {
            Some(next) => next(),
            None => Box::pin(async { Ok(None) }),
        }
    }

    fn with_target(mut self, target: Context) -> Self {
        self.target = Some(target);
        self
    }

    fn with_next(mut self, next: Next) -> Self {
        self.next = Some(next);
        self
    }
}

impl Debug for Event {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Event")
            .field("name", &self.name)
            .field("args", &self.args)
            .field("target", &self.target)
            .field("has_next", &self.next.is_some())
            .finish()
    }
}

type Callback = Arc<dyn Fn(Event) -> BoxFuture<EventResult> + Send + Sync + 'static>;

struct HookContext {
    root: Weak<RootInner>,
    fiber: Weak<FiberInner>,
    meta: ContextMeta,
}

impl HookContext {
    fn capture(ctx: &Context) -> Self {
        Self {
            root: Arc::downgrade(&ctx.root),
            fiber: ctx.fiber.clone(),
            meta: ctx.meta.clone(),
        }
    }

    fn get(&self) -> Option<Context> {
        Some(Context {
            root: self.root.upgrade()?,
            fiber: self.fiber.clone(),
            meta: self.meta.clone(),
        })
    }
}

struct Hook {
    id: u64,
    ctx: HookContext,
    callback: Callback,
    options: EventOptions,
    once: bool,
    effect: Option<Weak<crate::effect::EffectCell>>,
}

#[derive(Default)]
struct EventsState {
    hooks: HashMap<String, Vec<Hook>>,
}

pub(crate) struct EventsRoot {
    state: Mutex<EventsState>,
    next_hook: AtomicU64,
}

impl EventsRoot {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(EventsState::default()),
            next_hook: AtomicU64::new(0),
        }
    }

    fn next_id(&self) -> u64 {
        self.next_hook.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn remove(&self, name: &str, id: u64) -> bool {
        let mut state = lock(&self.state);
        let Some(hooks) = state.hooks.get_mut(name) else {
            return false;
        };
        let before = hooks.len();
        hooks.retain(|hook| hook.id != id);
        let removed = hooks.len() != before;
        if hooks.is_empty() {
            state.hooks.remove(name);
        }
        removed
    }
}

/// Event bus bound to a context.
#[derive(Clone, Debug)]
pub struct EventsService {
    ctx: Context,
}

impl EventsService {
    pub(crate) fn new(ctx: Context) -> Self {
        Self { ctx }
    }

    fn register_callback(
        &self,
        name: String,
        callback: Callback,
        options: EventOptions,
        once: bool,
    ) -> Result<EffectHandle> {
        let fiber = self.ctx.fiber()?;
        fiber.assert_active()?;
        let id = self.ctx.root.events.next_id();
        {
            let mut state = lock(&self.ctx.root.events.state);
            let hooks = state.hooks.entry(name.clone()).or_default();
            let hook = Hook {
                id,
                ctx: HookContext::capture(&self.ctx),
                callback,
                options,
                once,
                effect: None,
            };
            if options.prepend {
                hooks.insert(0, hook);
            } else {
                hooks.push(hook);
            }
        }

        let root = Arc::downgrade(&self.ctx.root);
        let effect_name = name.clone();
        let effect = fiber.register_effect(
            format!("ctx.on({name:?})"),
            AsyncDisposer::from_sync(move || {
                if let Some(root) = root.upgrade() {
                    root.events.remove(&effect_name, id);
                }
                Ok(())
            }),
        );
        match effect {
            Ok(effect) => {
                if let Some(hook) = lock(&self.ctx.root.events.state)
                    .hooks
                    .get_mut(&name)
                    .and_then(|hooks| hooks.iter_mut().find(|hook| hook.id == id))
                {
                    hook.effect = Some(Arc::downgrade(&effect.cell));
                }
                Ok(effect)
            }
            Err(error) => {
                self.ctx.root.events.remove(&name, id);
                Err(error)
            }
        }
    }

    /// Register a synchronous event listener owned by the current fiber.
    pub fn on<F>(
        &self,
        name: impl Into<String>,
        listener: F,
        options: EventOptions,
    ) -> Result<EffectHandle>
    where
        F: Fn(Event) -> EventResult + Send + Sync + 'static,
    {
        let listener = Arc::new(listener);
        self.register_callback(
            name.into(),
            Arc::new(move |event| {
                let listener = listener.clone();
                Box::pin(async move { listener(event) })
            }),
            options,
            false,
        )
    }

    /// Register an asynchronous event listener.
    pub fn on_async<F, Fut>(
        &self,
        name: impl Into<String>,
        listener: F,
        options: EventOptions,
    ) -> Result<EffectHandle>
    where
        F: Fn(Event) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = EventResult> + Send + 'static,
    {
        let listener = Arc::new(listener);
        self.register_callback(
            name.into(),
            Arc::new(move |event| Box::pin(listener(event))),
            options,
            false,
        )
    }

    /// Register a synchronous listener removed before its first invocation.
    pub fn once<F>(
        &self,
        name: impl Into<String>,
        listener: F,
        options: EventOptions,
    ) -> Result<EffectHandle>
    where
        F: Fn(Event) -> EventResult + Send + Sync + 'static,
    {
        let listener = Arc::new(listener);
        self.register_callback(
            name.into(),
            Arc::new(move |event| {
                let listener = listener.clone();
                Box::pin(async move { listener(event) })
            }),
            options,
            true,
        )
    }

    /// Number of listeners registered for `name`.
    pub fn listener_count(&self, name: &str) -> usize {
        lock(&self.ctx.root.events.state)
            .hooks
            .get(name)
            .map(Vec::len)
            .unwrap_or(0)
    }

    fn dispatch(&self, mode: DispatchMode, event: &Event) -> Vec<Callback> {
        if !event.name().starts_with("internal/") {
            let _ = self.emit(
                "internal/dispatch",
                [
                    Value::new(mode),
                    Value::new(event.name().to_owned()),
                    Value::new(event.args().to_vec()),
                ],
            );
        }

        let mut state = lock(&self.ctx.root.events.state);
        let Some(hooks) = state.hooks.get_mut(event.name()) else {
            return Vec::new();
        };
        let mut callbacks = Vec::new();
        let mut retained = Vec::with_capacity(hooks.len());
        for hook in std::mem::take(hooks) {
            let Some(owner) = hook.ctx.get() else {
                continue;
            };
            let accepted = hook.options.global
                || event
                    .target()
                    .and_then(Context::filter)
                    .map(|filter| filter(&owner))
                    .unwrap_or(true);
            if accepted {
                callbacks.push(hook.callback.clone());
                if hook.once {
                    if let Some(effect) = hook.effect.as_ref().and_then(Weak::upgrade) {
                        EffectHandle::new(effect).cancel();
                    }
                }
            }
            if !hook.once || !accepted {
                retained.push(hook);
            }
        }
        *hooks = retained;
        callbacks
    }

    /// Emit synchronously, returning the first listener error.
    pub fn emit(
        &self,
        name: impl Into<String>,
        args: impl IntoIterator<Item = EventValue>,
    ) -> Result<()> {
        self.emit_event(Event::new(name, args))
    }

    /// Emit with an explicit target context for listener filtering.
    pub fn emit_from(
        &self,
        target: Context,
        name: impl Into<String>,
        args: impl IntoIterator<Item = EventValue>,
    ) -> Result<()> {
        self.emit_event(Event::new(name, args).with_target(target))
    }

    fn emit_event(&self, event: Event) -> Result<()> {
        for callback in self.dispatch(DispatchMode::Emit, &event) {
            block_on(callback(event.clone()))?;
        }
        Ok(())
    }

    /// Run all listeners concurrently and aggregate failures.
    pub async fn parallel(
        &self,
        name: impl Into<String>,
        args: impl IntoIterator<Item = EventValue>,
    ) -> Result<()> {
        let event = Event::new(name, args);
        let futures = self
            .dispatch(DispatchMode::Parallel, &event)
            .into_iter()
            .map(|callback| callback(event.clone()))
            .collect();
        let errors = join_all(futures)
            .await
            .into_iter()
            .filter_map(|result| result.err())
            .collect::<Vec<_>>();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(CordisError::with_message(
                ErrorCode::Event,
                format!(
                    "{} event listener(s) failed: {}",
                    errors.len(),
                    errors
                        .into_iter()
                        .map(|error| error.to_string())
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
            ))
        }
    }

    /// Run listeners in order, awaiting each, until one bails.
    pub async fn serial(
        &self,
        name: impl Into<String>,
        args: impl IntoIterator<Item = EventValue>,
    ) -> EventResult {
        let event = Event::new(name, args);
        for callback in self.dispatch(DispatchMode::Serial, &event) {
            let result = callback(event.clone()).await?;
            if is_bailed(&result) {
                return Ok(result);
            }
        }
        Ok(None)
    }

    /// Synchronous ordered bail dispatch.
    pub fn bail(
        &self,
        name: impl Into<String>,
        args: impl IntoIterator<Item = EventValue>,
    ) -> EventResult {
        let event = Event::new(name, args);
        for callback in self.dispatch(DispatchMode::Bail, &event) {
            let result = block_on(callback(event.clone()))?;
            if is_bailed(&result) {
                return Ok(result);
            }
        }
        Ok(None)
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
        let event = Event::new(name, args);
        let callbacks = self.dispatch(DispatchMode::Waterfall, &event);
        let inner = Arc::new(inner);
        let mut next: Next = Arc::new(move || Box::pin(inner()));
        for callback in callbacks.into_iter().rev() {
            let previous = next.clone();
            let event = event.clone();
            next = Arc::new(move || {
                let callback = callback.clone();
                let current = event.clone().with_next(previous.clone());
                callback(current)
            });
        }
        next().await
    }

    /// Synchronous waterfall convenience wrapper.
    pub fn waterfall<F>(
        &self,
        name: impl Into<String>,
        args: impl IntoIterator<Item = EventValue>,
        inner: F,
    ) -> EventResult
    where
        F: Fn() -> EventResult + Send + Sync + 'static,
    {
        block_on(self.waterfall_async(name, args, move || {
            let result = inner();
            async move { result }
        }))
    }

    /// Remove every event hook. Primarily useful for diagnostics/tests; normal
    /// cleanup should dispose the owning fiber.
    pub fn clear(&self) {
        lock(&self.ctx.root.events.state).hooks.clear();
    }
}
