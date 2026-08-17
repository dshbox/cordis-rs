//! Fixture compiled to a `cdylib` by the dynamic tests: a plugin that
//! writes its build tag (`v1`, or `v2` under `--cfg hmr_v2`) to the file
//! configured as `out`.
//!
//! Compiled with the same toolchain and workspace rlibs as the test binary,
//! so its fingerprint satisfies the loader's check. Everything comes from
//! the `cordis_loader` prelude so a single `--extern` suffices.

use cordis_loader::Node;
use cordis_loader::dynamic::{BoxFuture, Config, Context, Plugin, PluginOutput, Result};

#[cfg(hmr_v2)]
const TEXT: &str = "v2";
#[cfg(not(hmr_v2))]
const TEXT: &str = "v1";

/// Writes [`TEXT`] to the configured `out` path on apply.
pub struct FileWriter;

impl Plugin for FileWriter {
    fn name(&self) -> &str {
        "file_writer"
    }

    fn apply(&self, _ctx: Context, config: Config) -> BoxFuture<Result<PluginOutput>> {
        Box::pin(async move {
            let node = config.downcast::<Node>()?;
            let out = node
                .as_object()
                .and_then(|object| object.get("out"))
                .and_then(Node::as_str)
                .unwrap_or("cordis-dynamic-fixture-out");
            std::fs::write(out, TEXT)
                .map_err(|error| format!("file_writer: cannot write {out}: {error}"))?;
            Ok(PluginOutput::none())
        })
    }
}

cordis_loader::dynamic::export_plugin!(FileWriter);
