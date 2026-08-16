//! Typed service conventions and helpers.

use crate::context::Context;
use crate::effect::EffectHandle;
use crate::registry::{Inject, PluginHandle, PluginOutput, plugin_async, plugin_sync};
use crate::{Result, Value};
use std::future::Future;
use std::sync::Arc;

/// Marker and availability contract for named Cordis services.
///
/// The TypeScript base class registers itself from its constructor. Rust does
/// not have inheritable constructors, so service values implement this trait
/// and are installed through [`Context::provide_service`]. Their lifetime is
/// still owned by the current plugin fiber.
pub trait Service: Send + Sync + 'static {
    /// Context property/service key.
    const NAME: &'static str;

    /// Whether dependent plugins may currently use this service.
    fn is_available(&self) -> bool {
        true
    }
}

impl Context {
    /// Provide a typed service under [`Service::NAME`].
    pub fn provide_service<S>(&self, service: S) -> Result<EffectHandle>
    where
        S: Service,
    {
        self.provide_service_arc(Arc::new(service))
    }

    /// Provide an existing shared typed service.
    pub fn provide_service_arc<S>(&self, service: Arc<S>) -> Result<EffectHandle>
    where
        S: Service,
    {
        let value = Value::from_arc(service);
        let check = Arc::new(|value: &Value| {
            value
                .as_any()
                .downcast_ref::<S>()
                .map(|service| service.is_available())
                .unwrap_or(false)
        });
        self.reflect()
            .provide_value(S::NAME.to_owned(), value, Some(check))
    }

    /// Resolve service intercepts from root to leaf with a caller-supplied
    /// merge operation.
    pub fn resolve_service_config<T, F>(&self, name: &str, base: T, mut merge: F) -> Result<T>
    where
        T: Send + Sync + 'static,
        F: FnMut(T, &T) -> T,
    {
        let mut output = base;
        for config in self.intercepts::<T>(name)? {
            output = merge(output, config.as_ref());
        }
        Ok(output)
    }
}

/// Build a synchronous class-service-style plugin.
///
/// The constructor's returned service is automatically provided and removed
/// with the plugin fiber.
pub fn service_sync<S, C, F>(
    name: impl Into<String>,
    inject: Inject,
    constructor: F,
) -> PluginHandle
where
    S: Service,
    C: Send + Sync + 'static,
    F: Fn(Context, Arc<C>) -> Result<S> + Send + Sync + 'static,
{
    plugin_sync(name, inject, move |ctx, config| {
        let service = constructor(ctx.clone(), config)?;
        ctx.provide_service(service)?;
        Ok(PluginOutput::none())
    })
}

/// Build an asynchronous class-service-style plugin.
pub fn service_async<S, C, F, Fut>(
    name: impl Into<String>,
    inject: Inject,
    constructor: F,
) -> PluginHandle
where
    S: Service,
    C: Send + Sync + 'static,
    F: Fn(Context, Arc<C>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<S>> + Send + 'static,
{
    let constructor = Arc::new(constructor);
    plugin_async(name, inject, move |ctx, config| {
        let constructor = constructor.clone();
        async move {
            let service = constructor(ctx.clone(), config).await?;
            ctx.provide_service(service)?;
            Ok(PluginOutput::none())
        }
    })
}
