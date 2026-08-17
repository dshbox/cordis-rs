//! Fixture compiled to a `cdylib` by the dynamic tests: a plugin whose
//! apply future panics, to prove poll-time panics are contained on the
//! plugin side and surface as a fiber error instead of aborting.

use cordis_loader::dynamic::{BoxFuture, Config, Context, Plugin, PluginOutput, Result};

/// Panics inside the apply future.
pub struct PanicInApply;

impl Plugin for PanicInApply {
    fn name(&self) -> &str {
        "panic_in_apply"
    }

    fn apply(&self, _ctx: Context, _config: Config) -> BoxFuture<Result<PluginOutput>> {
        Box::pin(async {
            panic!("boom from panic_in_apply");
        })
    }
}

cordis_loader::dynamic::export_plugin!(PanicInApply);
