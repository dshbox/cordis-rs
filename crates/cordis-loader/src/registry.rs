//! Static plugin registry — the Rust replacement for upstream's dynamic
//! `import(name)`.

use cordis::utils::BoxFuture;
use cordis::{Config, Context, Inject, Plugin, PluginHandle, PluginOutput, Result};
use cordis_group::Group;
use cordis_include::IMPORT_NAME;
use cordis_include::resolver::unknown_plugin;
use std::collections::HashMap;
#[cfg(feature = "dynamic")]
use std::path::PathBuf;
use std::sync::Arc;

/// A factory producing fresh plugin handles.
type Factory = Arc<dyn Fn() -> PluginHandle + Send + Sync>;

/// Name-to-factory plugin registry, populated at build or startup time.
///
/// Resolving the same name twice yields distinct [`PluginHandle`]s (and
/// thus distinct [`cordis::PluginKey`] identities), mirroring one plugin
/// instance per entry. The registry pre-registers [`cordis_group`]'s
/// `group` marker.
///
/// With the `dynamic` feature, a
/// [`DynamicPluginResolver`](crate::dynamic::DynamicPluginResolver) can be
/// attached as a fallback: names missing from the registry are then looked
/// up as dynamic libraries in its directories.
#[derive(Clone, Default)]
pub struct PluginRegistry {
    factories: HashMap<String, Factory>,
    #[cfg(feature = "dynamic")]
    dynamic: Option<crate::dynamic::DynamicPluginResolver>,
}

impl PluginRegistry {
    /// An empty registry with the `group` and `import` builtins registered.
    pub fn new() -> Self {
        let mut registry = Self::default();
        registry.register("group", Group::handle);
        registry.register(IMPORT_NAME, || PluginHandle::new(Import));
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

    /// Attach `resolver` as the fallback for unknown names (dynamic
    /// feature).
    #[cfg(feature = "dynamic")]
    pub fn set_dynamic(&mut self, resolver: crate::dynamic::DynamicPluginResolver) {
        self.dynamic = Some(resolver);
    }

    /// Builder form of [`PluginRegistry::set_dynamic`]: search `dirs` for
    /// dynamic-library plugins when a name is not statically registered.
    #[cfg(feature = "dynamic")]
    pub fn with_dynamic_dirs<I, D>(mut self, dirs: I) -> Self
    where
        I: IntoIterator<Item = D>,
        D: Into<PathBuf>,
    {
        self.set_dynamic(crate::dynamic::DynamicPluginResolver::new(dirs));
        self
    }

    /// The attached dynamic resolver, if any (dynamic feature).
    #[cfg(feature = "dynamic")]
    pub fn dynamic(&self) -> Option<&crate::dynamic::DynamicPluginResolver> {
        self.dynamic.as_ref()
    }
}

impl cordis_include::PluginResolver for PluginRegistry {
    fn resolve(&self, name: &str) -> Result<PluginHandle> {
        match self.factories.get(name) {
            Some(factory) => Ok(factory()),
            #[cfg(feature = "dynamic")]
            None => match &self.dynamic {
                Some(resolver) => resolver.resolve(name),
                None => Err(unknown_plugin(name)),
            },
            #[cfg(not(feature = "dynamic"))]
            None => Err(unknown_plugin(name)),
        }
    }
}

/// Nesting marker for import entries, whose children the loader mounts
/// from the referenced file at compose time.
pub(crate) struct Import;

impl Plugin for Import {
    fn name(&self) -> &str {
        IMPORT_NAME
    }

    fn apply(&self, _ctx: Context, _config: Config) -> BoxFuture<Result<PluginOutput>> {
        Box::pin(async { Ok(PluginOutput::default()) })
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
    /// Wrap `handle`, merging an entry's `inject` declaration into the
    /// plugin's own dependencies — the plugin's names first, then the
    /// entry's extras, deduplicated. An empty entry list adds nothing and
    /// keeps the bare handle.
    pub fn wrap(handle: PluginHandle, inject: Vec<String>) -> PluginHandle {
        if inject.is_empty() {
            return handle;
        }
        let mut names: Vec<String> = handle
            .plugin()
            .inject()
            .names()
            .map(ToString::to_string)
            .collect();
        for name in inject {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        PluginHandle::new(WithInject {
            handle,
            inject: Inject::new(names),
        })
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
