//! Fixture compiled to a `cdylib` by the dynamic tests: a plugin whose
//! `name()` panics, to prove panics are contained at the boundary instead
//! of unwinding through `extern "C"` or the loader.

use cordis_loader::dynamic::{BoxFuture, Config, Context, Plugin, PluginOutput, Result};

/// Panics in `name()`.
pub struct PanicOnName;

impl Plugin for PanicOnName {
    fn name(&self) -> &str {
        panic!("boom from panic_on_name")
    }

    fn apply(&self, _ctx: Context, _config: Config) -> BoxFuture<Result<PluginOutput>> {
        Box::pin(async { Ok(PluginOutput::none()) })
    }
}

cordis_loader::dynamic::export_plugin!(PanicOnName);
