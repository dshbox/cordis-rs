//! Scoped service storage and explicit reflection APIs.

use crate::context::{Context, Isolation, RootInner};
use crate::effect::{AsyncDisposer, EffectHandle};
use crate::fiber::{Fiber, FiberInner, FiberState};
use crate::registry::Inject;
use crate::utils::lock;
use crate::{CordisError, ErrorCode, Result, Value};
use std::collections::HashMap;
use std::fmt::{self, Debug, Formatter};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

/// Dynamic getter used by computed context properties.
pub type AccessorGet = Arc<dyn Fn(&Context) -> Result<Option<Value>> + Send + Sync + 'static>;
/// Dynamic setter used by computed context properties.
pub type AccessorSet = Arc<dyn Fn(&Context, Value) -> Result<()> + Send + Sync + 'static>;

/// Explicit replacement for a JavaScript proxy-backed computed property.
#[derive(Clone)]
pub struct Accessor {
    /// Getter callback.
    pub get: AccessorGet,
    /// Optional setter callback.
    pub set: Option<AccessorSet>,
}

impl Accessor {
    /// Construct a read-only accessor.
    pub fn read_only<F>(get: F) -> Self
    where
        F: Fn(&Context) -> Result<Option<Value>> + Send + Sync + 'static,
    {
        Self {
            get: Arc::new(get),
            set: None,
        }
    }

    /// Construct a read/write accessor.
    pub fn read_write<G, S>(get: G, set: S) -> Self
    where
        G: Fn(&Context) -> Result<Option<Value>> + Send + Sync + 'static,
        S: Fn(&Context, Value) -> Result<()> + Send + Sync + 'static,
    {
        Self {
            get: Arc::new(get),
            set: Some(Arc::new(set)),
        }
    }
}

impl Debug for Accessor {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Accessor")
            .field("writable", &self.set.is_some())
            .finish_non_exhaustive()
    }
}

/// Reflected property declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Property {
    /// Scoped service implementation.
    Service,
    /// Dynamic getter/setter.
    Accessor,
    /// Explicit alias to another service name (Rust replacement for mixins).
    Alias(String),
}

struct AccessorRecord {
    owner_uid: u64,
    accessor: Accessor,
}

type AvailabilityCheck = Arc<dyn Fn(&Value) -> bool + Send + Sync + 'static>;

pub(crate) struct ServiceImpl {
    name: String,
    value: Value,
    fiber: Weak<FiberInner>,
    provider_uid: u64,
    generation: u64,
    check: Option<AvailabilityCheck>,
}

#[derive(Default)]
struct ReflectState {
    default_scopes: HashMap<String, Isolation>,
    implementations: HashMap<Isolation, ServiceImpl>,
    properties: HashMap<String, Property>,
    accessors: HashMap<String, AccessorRecord>,
}

pub(crate) struct ReflectRoot {
    state: Mutex<ReflectState>,
    next_generation: AtomicU64,
}

impl ReflectRoot {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(ReflectState::default()),
            next_generation: AtomicU64::new(0),
        }
    }

    fn next_generation(&self) -> u64 {
        self.next_generation.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn scope_for(&self, ctx: &Context, name: &str) -> Option<Isolation> {
        ctx.scope_override(name)
            .or_else(|| lock(&self.state).default_scopes.get(name).copied())
    }

    fn ensure_scope(&self, ctx: &Context, name: &str) -> Isolation {
        if let Some(scope) = ctx.scope_override(name) {
            return scope;
        }
        let mut state = lock(&self.state);
        if let Some(scope) = state.default_scopes.get(name) {
            return *scope;
        }
        let scope = ctx.root.scope();
        state.default_scopes.insert(name.to_owned(), scope);
        scope
    }

    fn value(&self, ctx: &Context, name: &str, strict: bool) -> Result<Option<Value>> {
        // Scope overrides live in the lock-free context meta, so one guard
        // covers the whole lookup.
        let scope_override = ctx.scope_override(name);
        let state = lock(&self.state);
        match state.properties.get(name).cloned() {
            Some(Property::Accessor) => {
                let accessor = state
                    .accessors
                    .get(name)
                    .map(|record| record.accessor.clone());
                drop(state);
                return match accessor {
                    Some(accessor) => (accessor.get)(ctx),
                    None => Ok(None),
                };
            }
            Some(Property::Alias(target)) => {
                drop(state);
                return self.value(ctx, &target, strict);
            }
            _ => {}
        }

        let Some(scope) = scope_override.or_else(|| state.default_scopes.get(name).copied()) else {
            return Ok(None);
        };
        let Some(implementation) = state.implementations.get(&scope) else {
            return Ok(None);
        };
        if implementation.name != name {
            return Ok(None);
        }
        let value = implementation.value.clone();
        let provider = implementation.fiber.upgrade();
        let provider_uid = implementation.provider_uid;
        drop(state);

        let Some(provider) = provider else {
            return Ok(None);
        };
        if strict {
            let caller_uid = ctx.fiber().ok().and_then(|fiber| fiber.uid());
            if caller_uid != Some(provider_uid)
                && Fiber::from_inner(provider).state() != FiberState::Active
            {
                return Ok(None);
            }
        }
        Ok(Some(value))
    }

    fn implementation_epoch(&self, ctx: &Context, name: &str) -> Option<u64> {
        let scope_override = ctx.scope_override(name);
        let (generation, provider, check, value) = {
            let state = lock(&self.state);
            let scope = scope_override.or_else(|| state.default_scopes.get(name).copied())?;
            let implementation = state.implementations.get(&scope)?;
            if implementation.name != name {
                return None;
            }
            (
                implementation.generation,
                implementation.fiber.upgrade(),
                implementation.check.clone(),
                // The value only serves the availability check; cloning it
                // when no check is registered is a wasted Arc round-trip per
                // dependency per fiber reconcile.
                implementation
                    .check
                    .as_ref()
                    .map(|_| implementation.value.clone()),
            )
        };
        let provider = provider?;
        if Fiber::from_inner(provider).state() != FiberState::Active {
            return None;
        }
        if let (Some(check), Some(value)) = (check, value) {
            if !check(&value) {
                return None;
            }
        }
        Some(generation)
    }

    fn services_owned_by(&self, uid: u64) -> Vec<(String, Isolation)> {
        lock(&self.state)
            .implementations
            .iter()
            .filter(|(_, implementation)| implementation.provider_uid == uid)
            .map(|(scope, implementation)| (implementation.name.clone(), *scope))
            .collect()
    }
}

/// Public service implementation diagnostics.
#[derive(Debug, Clone)]
pub struct ServiceInfo {
    /// Service name.
    pub name: String,
    /// Scope label.
    pub isolation: Isolation,
    /// Provider fiber id.
    pub provider_uid: u64,
    /// Provider display name.
    pub provider_name: String,
    /// Concrete Rust type name.
    pub type_name: &'static str,
    /// Generation used in dependency epochs.
    pub generation: u64,
}

/// Reflection and service-resolution API bound to a context.
#[derive(Clone, Debug)]
pub struct ReflectService {
    ctx: Context,
}

impl ReflectService {
    pub(crate) fn new(ctx: Context) -> Self {
        Self { ctx }
    }

    /// Read and downcast a service.
    pub fn get<T>(&self, name: &str, strict: bool) -> Result<Option<Arc<T>>>
    where
        T: Send + Sync + 'static,
    {
        self.get_value(name, strict)?
            .map(|value| value.downcast())
            .transpose()
    }

    /// Read a type-erased service or accessor result.
    pub fn get_value(&self, name: &str, strict: bool) -> Result<Option<Value>> {
        self.ctx.root.reflect.value(&self.ctx, name, strict)
    }

    /// Require and downcast an active service.
    pub fn require<T>(&self, name: &str) -> Result<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        let value = self.get_value(name, true)?.ok_or_else(|| {
            CordisError::with_message(
                ErrorCode::MissingService,
                format!("required service \"{name}\" is unavailable"),
            )
        })?;
        value.downcast()
    }

    /// Register a type-erased service and optional availability predicate.
    pub fn provide_value(
        &self,
        name: String,
        value: Value,
        check: Option<AvailabilityCheck>,
    ) -> Result<EffectHandle> {
        let fiber = self.ctx.fiber()?;
        fiber.assert_active()?;
        let uid = fiber
            .uid()
            .ok_or_else(|| CordisError::new(ErrorCode::InactiveEffect))?;
        let scope = self.ctx.root.reflect.ensure_scope(&self.ctx, &name);
        let generation = self.ctx.root.reflect.next_generation();

        {
            let mut state = lock(&self.ctx.root.reflect.state);
            if let Some(property) = state.properties.get(&name) {
                if *property != Property::Service {
                    return Err(CordisError::with_message(
                        ErrorCode::PropertyConflict,
                        format!("property \"{name}\" is already declared as {property:?}"),
                    ));
                }
            }
            if let Some(existing) = state.implementations.get(&scope) {
                let provider_name = existing
                    .fiber
                    .upgrade()
                    .map(Fiber::from_inner)
                    .map(|fiber| fiber.name())
                    .unwrap_or_else(|| "disposed".to_owned());
                return Err(CordisError::with_message(
                    ErrorCode::DuplicateService,
                    format!("service \"{name}\" has been registered at <{provider_name}>"),
                ));
            }
            state.properties.insert(name.clone(), Property::Service);
            state.implementations.insert(
                scope,
                ServiceImpl {
                    name: name.clone(),
                    value,
                    fiber: Arc::downgrade(&fiber.inner),
                    provider_uid: uid,
                    generation,
                    check,
                },
            );
        }

        let root = Arc::downgrade(&self.ctx.root);
        let effect_name = name.clone();
        let effect = fiber.register_effect(
            format!("ctx.provide({name:?})"),
            AsyncDisposer::from_sync(move || {
                let Some(root) = root.upgrade() else {
                    return Ok(());
                };
                let removed = {
                    let mut state = lock(&root.reflect.state);
                    match state.implementations.get(&scope) {
                        Some(current) if current.generation == generation => {
                            state.implementations.remove(&scope);
                            true
                        }
                        _ => false,
                    }
                };
                if removed {
                    root.notify_service(&effect_name, scope);
                }
                Ok(())
            }),
        );

        match effect {
            Ok(effect) => {
                self.ctx.root.notify_service(&name, scope);
                Ok(effect)
            }
            Err(error) => {
                lock(&self.ctx.root.reflect.state)
                    .implementations
                    .remove(&scope);
                Err(error)
            }
        }
    }

    /// Register a service with an availability predicate.
    pub fn provide_checked<T, F>(
        &self,
        name: impl Into<String>,
        value: T,
        check: F,
    ) -> Result<EffectHandle>
    where
        T: Send + Sync + 'static,
        F: Fn(&T) -> bool + Send + Sync + 'static,
    {
        let value = Value::new(value);
        let check = Arc::new(move |value: &Value| {
            value
                .as_any()
                .downcast_ref::<T>()
                .map(&check)
                .unwrap_or(false)
        });
        self.provide_value(name.into(), value, Some(check))
    }

    /// Replace a service or computed property value.
    pub fn set_value(&self, name: &str, value: Value) -> Result<()> {
        let scope_override = self.ctx.scope_override(name);
        let mut state = lock(&self.ctx.root.reflect.state);
        if state.properties.get(name) == Some(&Property::Accessor) {
            let setter = state
                .accessors
                .get(name)
                .and_then(|record| record.accessor.set.clone());
            drop(state);
            let setter = setter.ok_or_else(|| {
                CordisError::with_message(
                    ErrorCode::AccessDenied,
                    format!("property \"{name}\" is read-only"),
                )
            })?;
            return setter(&self.ctx, value);
        }

        let scope = scope_override
            .or_else(|| state.default_scopes.get(name).copied())
            .ok_or_else(|| {
                CordisError::with_message(
                    ErrorCode::MissingService,
                    format!("cannot set property \"{name}\" without provide"),
                )
            })?;
        let uid = self.ctx.fiber()?.uid();
        let implementation = state.implementations.get_mut(&scope).ok_or_else(|| {
            CordisError::with_message(
                ErrorCode::MissingService,
                format!("cannot set property \"{name}\" without provide"),
            )
        })?;
        if uid != Some(implementation.provider_uid) {
            return Err(CordisError::with_message(
                ErrorCode::AccessDenied,
                format!("cannot set property \"{name}\" in multiple fibers"),
            ));
        }
        implementation.value = value;
        Ok(())
    }

    /// Define a dynamic computed context property.
    pub fn accessor(&self, name: String, accessor: Accessor) -> Result<EffectHandle> {
        let fiber = self.ctx.fiber()?;
        let uid = fiber
            .uid()
            .ok_or_else(|| CordisError::new(ErrorCode::InactiveEffect))?;
        {
            let mut state = lock(&self.ctx.root.reflect.state);
            if let Some(property) = state.properties.get(&name) {
                return Err(CordisError::with_message(
                    ErrorCode::PropertyConflict,
                    format!("property \"{name}\" is already declared as {property:?}"),
                ));
            }
            state.properties.insert(name.clone(), Property::Accessor);
            state.accessors.insert(
                name.clone(),
                AccessorRecord {
                    owner_uid: uid,
                    accessor,
                },
            );
        }
        let root = Arc::downgrade(&self.ctx.root);
        let effect_name = name.clone();
        let effect = fiber.register_effect(
            format!("ctx.accessor({name:?})"),
            AsyncDisposer::from_sync(move || {
                if let Some(root) = root.upgrade() {
                    let mut state = lock(&root.reflect.state);
                    let owned = state
                        .accessors
                        .get(&effect_name)
                        .map(|record| record.owner_uid == uid)
                        .unwrap_or(false);
                    if owned {
                        state.accessors.remove(&effect_name);
                        state.properties.remove(&effect_name);
                    }
                }
                Ok(())
            }),
        );
        if effect.is_err() {
            let mut state = lock(&self.ctx.root.reflect.state);
            state.accessors.remove(&name);
            state.properties.remove(&name);
        }
        effect
    }

    /// Alias one reflected service name to another. This is the explicit Rust
    /// counterpart of the original dynamic `mixin()` forwarding.
    pub fn alias(
        &self,
        alias: impl Into<String>,
        target: impl Into<String>,
    ) -> Result<EffectHandle> {
        let alias = alias.into();
        let target = target.into();
        let getter_target = target.clone();
        let setter_target = target;
        self.accessor(
            alias,
            Accessor::read_write(
                move |ctx| ctx.reflect().get_value(&getter_target, true),
                move |ctx, value| ctx.reflect().set_value(&setter_target, value),
            ),
        )
    }

    /// Re-evaluate fibers that inject any of the named services.
    ///
    /// This is useful after an availability predicate registered with
    /// [`provide_checked`](Self::provide_checked) changes without replacing
    /// the service value. The returned fibers were in a matching isolation
    /// scope and were asked to reconcile.
    pub fn notify<I, S>(&self, names: I) -> Vec<Fiber>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let scoped_names = names
            .into_iter()
            .filter_map(|name| {
                let name = name.as_ref().to_owned();
                self.ctx
                    .root
                    .reflect
                    .scope_for(&self.ctx, &name)
                    .map(|scope| (name, scope))
            })
            .collect::<Vec<_>>();
        let mut affected: Vec<Fiber> = Vec::new();
        for (name, scope) in scoped_names {
            for fiber in self.ctx.root.notify_service(&name, scope) {
                if !affected
                    .iter()
                    .any(|seen| Arc::ptr_eq(&seen.inner, &fiber.inner))
                {
                    affected.push(fiber);
                }
            }
        }
        affected
    }

    /// List reflected property declarations.
    pub fn properties(&self) -> HashMap<String, Property> {
        lock(&self.ctx.root.reflect.state).properties.clone()
    }

    /// List concrete service implementations in all isolation scopes.
    pub fn services(&self) -> Vec<ServiceInfo> {
        lock(&self.ctx.root.reflect.state)
            .implementations
            .iter()
            .map(|(scope, implementation)| ServiceInfo {
                name: implementation.name.clone(),
                isolation: *scope,
                provider_uid: implementation.provider_uid,
                provider_name: implementation
                    .fiber
                    .upgrade()
                    .map(Fiber::from_inner)
                    .map(|fiber| fiber.name())
                    .unwrap_or_else(|| "disposed".to_owned()),
                type_name: implementation.value.type_name(),
                generation: implementation.generation,
            })
            .collect()
    }
}

impl RootInner {
    pub(crate) fn dependency_epoch(&self, ctx: &Context, inject: &Inject) -> Option<Vec<u64>> {
        // Generations aligned with inject order. Names are never read, only
        // compared for equality, so cloning them per refresh is pure waste.
        let mut epoch = Vec::with_capacity(inject.len());
        for dependency in inject.iter() {
            epoch.push(self.reflect.implementation_epoch(ctx, &dependency.name)?);
        }
        Some(epoch)
    }

    pub(crate) fn notify_service(&self, name: &str, scope: Isolation) -> Vec<Fiber> {
        let mut refreshed = Vec::new();
        for fiber in self.registry.fibers_injecting(name) {
            let same_scope = fiber
                .scope_override(name)
                .or_else(|| lock(&self.reflect.state).default_scopes.get(name).copied())
                == Some(scope);
            if same_scope {
                fiber.refresh();
                refreshed.push(fiber);
            }
        }

        if let Some(root) = self.root_fiber.get().and_then(Fiber::context) {
            let mut args = vec![Value::new(name.to_owned())];
            if let Ok(Some(value)) = self.reflect.value(&root, name, false) {
                args.push(value);
            }
            if let Err(error) = root.events().emit("internal/service", args) {
                root.log_error(error);
            }
        }
        refreshed
    }

    pub(crate) fn notify_fiber_services(&self, fiber: &Fiber) {
        let Some(uid) = fiber.uid() else {
            return;
        };
        for (name, scope) in self.reflect.services_owned_by(uid) {
            self.notify_service(&name, scope);
        }
    }
}
