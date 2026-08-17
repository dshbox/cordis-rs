//! Static plugin registry — the Rust replacement for upstream's dynamic
//! `import(name)`.

use cordis::utils::BoxFuture;
use cordis::{Config, Context, Inject, Plugin, PluginHandle, PluginOutput, Result};
use cordis_group::Group;
use cordis_include::resolver::unknown_plugin;
use std::collections::HashMap;
use std::sync::Arc;

/// A factory producing fresh plugin handles.
type Factory = Arc<dyn Fn() -> PluginHandle + Send + Sync>;

/// Name-to-factory plugin registry, populated at build or startup time.
///
/// Resolving the same name twice yields distinct [`PluginHandle`]s (and
/// thus distinct [`cordis::PluginKey`] identities), mirroring one plugin
/// instance per entry. The registry pre-registers [`cordis_group`]'s
/// `group` marker.
#[derive(Clone, Default)]
pub struct PluginRegistry {
    factories: HashMap<String, Factory>,
}

impl PluginRegistry {
    /// An empty registry with the `group` builtin registered.
    pub fn new() -> Self {
        let mut registry = Self::default();
        registry.register("group", Group::handle);
        registry
    }

    /// Register a handle factory under `name`.
    pub fn register<F>(&mut self, name: impl Into<String>, factory: F)
    where
        F: Fn() -> PluginHandle + Send + Sync + 'static,
    {
        self.factories.insert(name.into(), Arc::new(factory));
    }

    /// Register one plugin instance under its own name; every resolve then
    /// wraps a shared clone of it in a fresh handle.
    pub fn register_plugin<P: Plugin>(&mut self, plugin: P) {
        let name = plugin.name().to_owned();
        let shared = Arc::new(plugin);
        self.register(name, move || {
            PluginHandle::new(SharedPlugin(shared.clone()))
        });
    }

    /// Registered names, unordered.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.factories.keys().map(String::as_str)
    }
}

impl cordis_include::PluginResolver for PluginRegistry {
    fn resolve(&self, name: &str) -> Result<PluginHandle> {
        match self.factories.get(name) {
            Some(factory) => Ok(factory()),
            None => Err(unknown_plugin(name)),
        }
    }
}

/// Wraps one shared plugin instance so clones produce distinct handles.
struct SharedPlugin<P>(Arc<P>);

impl<P: Plugin> Plugin for SharedPlugin<P> {
    fn name(&self) -> &str {
        self.0.name()
    }

    fn inject(&self) -> &Inject {
        self.0.inject()
    }

    fn validate_config(&self, config: Config) -> Result<Config> {
        self.0.validate_config(config)
    }

    fn apply(&self, ctx: Context, config: Config) -> BoxFuture<Result<PluginOutput>> {
        self.0.apply(ctx, config)
    }
}

/// Wraps a resolved plugin, merging an entry's `inject` declaration into the
/// plugin's own dependencies.
///
/// With this in place, the core fiber machinery already reconciles entries
/// when an injected service goes away or comes back — no loader-side batch
/// refresh needed.
pub(crate) struct WithInject {
    handle: PluginHandle,
    inject: Inject,
}

impl WithInject {
    /// Wrap `handle` unless `inject` is empty (nothing to add).
    pub fn wrap(handle: PluginHandle, inject: Vec<String>) -> PluginHandle {
        if inject.is_empty() {
            handle
        } else {
            PluginHandle::new(WithInject {
                handle,
                inject: Inject::new(inject),
            })
        }
    }
}

impl Plugin for WithInject {
    fn name(&self) -> &str {
        self.handle.name()
    }

    fn inject(&self) -> &Inject {
        &self.inject
    }

    fn validate_config(&self, config: Config) -> Result<Config> {
        self.handle.plugin().validate_config(config)
    }

    fn apply(&self, ctx: Context, config: Config) -> BoxFuture<Result<PluginOutput>> {
        self.handle.plugin().apply(ctx, config)
    }
}
