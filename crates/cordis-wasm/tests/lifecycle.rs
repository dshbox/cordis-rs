//! Integration proof that a replacement artifact receives a fresh Fiber.

use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

use cordis_core::event::observer_sync;
use cordis_core::logger::BufferExporter;
use cordis_core::{Context, FiberState, Level, Plugin, PreparedChange, PreparedPlugin, Routing};
use cordis_wasm::{ComponentArtifact, ComponentEvent, ComponentPlugin, HostEvent};

fn guest_component(fixture_name: &str, artifact_name: &str) -> PathBuf {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(fixture_name);
    let status = Command::new(env!("CARGO"))
        .args([
            "build",
            "--manifest-path",
            fixture
                .join("Cargo.toml")
                .to_str()
                .expect("UTF-8 fixture path"),
            "--target",
            "wasm32-wasip2",
        ])
        .status()
        .expect("guest component build starts");
    assert!(status.success(), "guest component build succeeds");
    fixture
        .join("target/wasm32-wasip2/debug")
        .join(artifact_name)
}

fn v1_component() -> &'static Path {
    static COMPONENT: OnceLock<PathBuf> = OnceLock::new();
    COMPONENT
        .get_or_init(|| guest_component("lifecycle-guest", "cordis_wasm_lifecycle_guest.wasm"))
        .as_path()
}

fn v2_component() -> &'static Path {
    static COMPONENT: OnceLock<PathBuf> = OnceLock::new();
    COMPONENT
        .get_or_init(|| {
            guest_component("lifecycle-guest-v2", "cordis_wasm_lifecycle_guest_v2.wasm")
        })
        .as_path()
}

fn non_cooperative_component() -> &'static Path {
    static COMPONENT: OnceLock<PathBuf> = OnceLock::new();
    COMPONENT
        .get_or_init(|| {
            guest_component(
                "lifecycle-guest-non-cooperative",
                "cordis_wasm_lifecycle_guest_non_cooperative.wasm",
            )
        })
        .as_path()
}

fn input(plugin: &ComponentPlugin, component: &Path) -> cordis_wasm::ComponentInput {
    plugin
        .prepare(ComponentArtifact::from_bytes(
            std::fs::read(component).expect("compiled guest is readable"),
        ))
        .expect("guest component prepares")
}

#[tokio::test]
async fn component_plugin_bridges_its_declared_events_and_configuration() {
    let plugin = ComponentPlugin::new().expect("the local Wasmtime engine is constructible");
    let artifact = ComponentArtifact::from_bytes(
        std::fs::read(v1_component()).expect("compiled guest is readable"),
    )
    .with_configuration(b"revision=v1".to_vec());
    let input = plugin.prepare(artifact).expect("guest component prepares");
    let context = Context::new();
    let logs = Arc::new(BufferExporter::new(16, Level::Debug).expect("valid log buffer"));
    let _logs = context
        .add_exporter(logs.clone())
        .expect("root admits diagnostic exporter");
    let events = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let observed_events = events.clone();
    let _events = context
        .on::<ComponentEvent, _>(observer_sync(move |_, event| {
            observed_events.lock().push(event);
            Ok::<_, Infallible>(())
        }))
        .expect("root admits Component event observer");

    let fiber = context
        .spawn(PreparedPlugin::from_input(plugin, input))
        .await
        .expect("guest component activates");
    context
        .emit::<HostEvent>(Routing::Unscoped, HostEvent::new("fixture/inbound", []))
        .await
        .expect("declared host event is delivered");
    fiber.dispose().await.expect("guest component disposes");
    context
        .emit::<HostEvent>(Routing::Unscoped, HostEvent::new("fixture/inbound", []))
        .await
        .expect("post-disposal host event dispatches harmlessly");

    assert_eq!(
        *events.lock(),
        vec![ComponentEvent::new(
            "fixture/activated",
            b"revision=v1".to_vec()
        )],
        "the guest's outbound event crosses only while its generation is active"
    );
    let texts = logs
        .snapshot()
        .into_iter()
        .map(|record| record.text().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        texts,
        [
            "guest manifest read",
            "guest configuration: revision=v1",
            "guest lifecycle activated",
            "guest received: fixture/inbound",
            "guest lifecycle disposed",
        ],
        "the declared subscription is removed with the Component generation"
    );
}

#[tokio::test]
async fn era_swap_disposes_v1_before_activating_v2_with_a_fresh_fiber() {
    let plugin = ComponentPlugin::new().expect("the local Wasmtime engine is constructible");
    let v1 = input(&plugin, v1_component());
    let v2 = input(&plugin, v2_component());
    let context = Context::new();
    let logs = Arc::new(BufferExporter::new(16, Level::Debug).expect("valid log buffer"));
    let _logs = context
        .add_exporter(logs.clone())
        .expect("root admits diagnostic exporter");

    let source = context
        .spawn(PreparedPlugin::from_input(plugin, v1))
        .await
        .expect("v1 activates");
    let source_id = source.id();
    let successor = source
        .era_swap(PreparedChange::from_input::<ComponentPlugin>(v2))
        .await
        .expect("v2 replaces v1");

    assert_eq!(source.state(), FiberState::Disposed);
    assert_eq!(successor.state(), FiberState::Active);
    assert_ne!(
        source_id,
        successor.id(),
        "era replacement allocates a new FiberId"
    );
    successor.dispose().await.expect("v2 disposes");
    let texts = logs
        .snapshot()
        .into_iter()
        .map(|record| record.text().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        texts,
        [
            "guest manifest read",
            "guest configuration: ",
            "guest lifecycle activated",
            "guest lifecycle disposed",
            "v2 activate",
            "v2 dispose",
        ]
    );
}

#[tokio::test]
async fn non_cooperative_guest_is_interrupted_before_it_can_retain_a_fiber() {
    let plugin = ComponentPlugin::new().expect("the local Wasmtime engine is constructible");
    let context = Context::new();
    let fiber = context
        .spawn(PreparedPlugin::from_input(
            plugin.clone(),
            input(&plugin, non_cooperative_component()),
        ))
        .await
        .expect("the guest activates");

    fiber
        .dispose()
        .await
        .expect("the epoch deadline interrupts the guest cleanup loop");
    assert_eq!(fiber.state(), FiberState::Disposed);
}
