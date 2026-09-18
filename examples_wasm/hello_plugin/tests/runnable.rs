//! Executable evidence for the Wasm `hello_plugin` extraction.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

use cordis_wasm::{ComponentArtifact, ComponentPlugin, HostEvent};
use cordis_core::logger::BufferExporter;
use cordis_core::{Context, Level, Plugin, PreparedPlugin, Routing};

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
                .expect("hello guest build starts");
            assert!(status.success(), "hello guest build succeeds");
            crate_dir.join("target/wasm32-wasip2/debug/cordis_wasm_hello_plugin.wasm")
        })
        .as_path()
}

#[tokio::test]
async fn hello_is_delivered_while_live_and_absent_after_teardown() {
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

    context
        .emit::<HostEvent>(Routing::Unscoped, HostEvent::new("ping", b"world"))
        .await
        .expect("live guest receives ping");
    fiber.dispose().await.expect("guest disposes");
    context
        .emit::<HostEvent>(Routing::Unscoped, HostEvent::new("ping", b"again"))
        .await
        .expect("post-disposal dispatch is harmless");

    assert_eq!(
        logs.snapshot()
            .into_iter()
            .map(|record| record.text().to_owned())
            .collect::<Vec<_>>(),
        [
            "hello plugin activated",
            "hello, world",
            "hello plugin disposed",
        ]
    );
}
