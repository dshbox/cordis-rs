//! Plugin name resolution contract, implemented by the loader layer.

use cordis::{CordisError, ErrorCode, PluginHandle};

/// Maps entry names to reusable plugin handles.
///
/// This is the Rust replacement for upstream Cordis' dynamic
/// `import(name)`: resolution is static and injectable. The default
/// implementation in `cordis-loader` is a registry populated at startup;
/// tests and embedders can supply their own (plain closures implement the
/// trait too).
///
/// The trait returns [`PluginHandle`] — not a bare plugin — because the
/// handle's stable `PluginKey` identity is what fibers, events, and "did
/// this plugin dispose itself?" checks key off. Resolving the same name
/// twice must yield distinct handles (wrap the plugin each time, as
/// `PluginHandle::new` does), mirroring one plugin instance per entry.
pub trait PluginResolver: Send + Sync + 'static {
    /// Resolve an entry's plugin name to a fresh handle.
    fn resolve(&self, name: &str) -> cordis::Result<PluginHandle>;
}

impl<F> PluginResolver for F
where
    F: Fn(&str) -> cordis::Result<PluginHandle> + Send + Sync + 'static,
{
    fn resolve(&self, name: &str) -> cordis::Result<PluginHandle> {
        self(name)
    }
}

/// The error returned for a name no resolver knows about.
pub fn unknown_plugin(name: &str) -> CordisError {
    CordisError::with_message(
        ErrorCode::MissingService,
        format!("no plugin registered under the name `{name}`"),
    )
}
