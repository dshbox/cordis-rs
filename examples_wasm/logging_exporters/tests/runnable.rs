//! Executable evidence for the Wasm `logging_exporters` extraction.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

use cordis_wasm::{ComponentArtifact, ComponentPlugin};
use cordis_core::logger::BufferExporter;
use cordis_core::{Context, Level, Plugin, PreparedPlugin};

fn guest_component() -> &'static Path {
    static COMPONENT: OnceLock<PathBuf> = OnceLock::new();
    COMPONENT
        .get_or_init(|| {
            let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
            let status = Command::new(env!("CARGO"))
                .args([
                    "build",
                    "--locked",
                    "--manifest-path",
                    crate_dir
                        .join("Cargo.toml")
                        .to_str()
                        .expect("UTF-8 manifest path"),
                    "--target",
                    "wasm32-wasip2",
                ])
                .status()
                .expect("logging guest build starts");
            assert!(status.success(), "logging guest build succeeds");
            crate_dir.join("target/wasm32-wasip2/debug/cordis_wasm_logging_exporters.wasm")
        })
        .as_path()
}

#[tokio::test]
async fn diagnostics_use_the_host_channel_and_host_exporter_policy() {
    let plugin = ComponentPlugin::new().expect("the local Wasmtime engine is constructible");
    let artifact = ComponentArtifact::from_bytes(
        std::fs::read(guest_component()).expect("compiled guest is readable"),
    );
    let input = plugin.prepare(artifact).expect("guest component prepares");
    let context = Context::new();
    let logs = Arc::new(BufferExporter::new(8, Level::Debug).expect("valid log buffer"));
    let _logs = context
        .add_exporter(logs.clone())
        .expect("root admits diagnostic exporter");
    let fiber = context
        .spawn(PreparedPlugin::from_input(plugin, input))
        .await
        .expect("guest activates");
    fiber.dispose().await.expect("guest disposes");

    let records = logs.snapshot();
    assert_eq!(
        records
            .iter()
            .map(|record| record.text())
            .collect::<Vec<_>>(),
        [
            "guest debug diagnostic",
            "guest info diagnostic",
            "guest warning diagnostic",
            "guest error diagnostic",
            "guest diagnostics disposed",
        ]
    );
    assert!(
        records
            .iter()
            .all(|record| record.channel() == "wasm-component"),
        "only the host assigns the Component's logger channel"
    );
}
