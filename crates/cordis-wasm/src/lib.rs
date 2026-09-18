//! Experimental Component Model integration for Cordis.
//!
//! One guest component instance belongs to one Cordis apply generation. The
//! adapter activates it during [`cordis_core::Plugin::apply`] and binds its
//! asynchronous guest cleanup to that generation through
//! [`cordis_core::effect`].

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cordis_core::effect::EffectRegistrationError;
use cordis_core::event::{ListenerRegistrationError, observer};
use cordis_core::{Context, Logger, Plugin};
use parking_lot::Mutex as ParkingMutex;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

const EPOCH_TICK: Duration = Duration::from_millis(10);
const GUEST_DEADLINE_TICKS: u64 = 50;
const INITIALIZATION_DEADLINE_TICKS: u64 = 100_000;

/// Advances one engine's epoch independently of Cordis's completion runtime.
struct EpochTicker {
    running: Arc<AtomicBool>,
    join: ParkingMutex<Option<JoinHandle<()>>>,
}

impl EpochTicker {
    fn start(engine: Arc<Engine>) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let thread_running = running.clone();
        let join = thread::spawn(move || {
            while thread_running.load(Ordering::Acquire) {
                thread::sleep(EPOCH_TICK);
                engine.increment_epoch();
            }
        });
        Self {
            running,
            join: ParkingMutex::new(Some(join)),
        }
    }
}

impl Drop for EpochTicker {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(join) = self.join.get_mut().take() {
            let _ = join.join();
        }
    }
}

#[allow(
    missing_docs,
    reason = "Wasmtime generates bindings from the documented WIT contract"
)]
pub(crate) mod bindings {
    wasmtime::component::bindgen!({
        path: "../../wit",
        world: "cordis-plugin",
    });
}

use bindings::CordisPlugin;

/// Standard host capabilities available to guest Components.
mod capabilities;

pub use capabilities::{ComponentEvent, HostEvent};

/// A reusable Component Model Plugin factory.
///
/// The factory owns one Wasmtime engine while each [`ComponentInput`] owns one
/// compiled guest artifact. Each call to [`Plugin::apply`] creates a fresh
/// guest instance for the current Cordis apply generation.
#[derive(Clone)]
pub struct ComponentPlugin {
    engine: Arc<Engine>,
    linker: Arc<Linker<HostState>>,
    _epoch_ticker: Arc<EpochTicker>,
}

impl ComponentPlugin {
    /// Construct an engine configured for async Component Model calls.
    pub fn new() -> Result<Self, EngineError> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.wasm_component_model_async(true);
        config.epoch_interruption(true);
        let engine = Arc::new(Engine::new(&config).map_err(EngineError::Create)?);
        let mut linker = Linker::new(&engine);
        capabilities::add_to_linker(&mut linker).map_err(EngineError::Linker)?;
        wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(EngineError::Linker)?;
        Ok(Self {
            _epoch_ticker: Arc::new(EpochTicker::start(engine.clone())),
            engine,
            linker: Arc::new(linker),
        })
    }
}

/// Failure while constructing a [`ComponentPlugin`] engine.
#[derive(Debug, Error)]
pub enum EngineError {
    /// Wasmtime rejected the requested Component Model configuration.
    #[error("could not create the Component Model engine: {0}")]
    Create(#[source] wasmtime::Error),
    /// The fixed host capability surface could not be linked.
    #[error("could not link the Component Model host capabilities: {0}")]
    Linker(#[source] wasmtime::Error),
}

/// Source bytes for one immutable guest component artifact.
#[derive(Clone)]
pub struct ComponentArtifact {
    bytes: Arc<[u8]>,
    digest: [u8; 32],
    configuration: Arc<[u8]>,
}

impl ComponentArtifact {
    /// Retain one component artifact's bytes for synchronous preparation.
    pub fn from_bytes(bytes: impl Into<Arc<[u8]>>) -> Self {
        let bytes = bytes.into();
        Self {
            digest: Sha256::digest(&bytes).into(),
            bytes,
            configuration: Arc::from([]),
        }
    }

    /// Retain an artifact with an externally supplied digest for admission checks.
    pub fn with_digest(bytes: impl Into<Arc<[u8]>>, digest: [u8; 32]) -> Self {
        Self {
            bytes: bytes.into(),
            digest,
            configuration: Arc::from([]),
        }
    }

    /// Attach immutable configuration for every generation using this artifact.
    pub fn with_configuration(mut self, bytes: impl Into<Arc<[u8]>>) -> Self {
        self.configuration = bytes.into();
        self
    }
}

/// A prepared, compiled component artifact.
#[derive(Clone)]
pub struct ComponentInput {
    component: Arc<Component>,
    configuration: Arc<[u8]>,
}

/// Failure while compiling a [`ComponentArtifact`] before lifecycle admission.
#[derive(Debug, Error)]
pub enum ComponentPrepareError {
    /// The artifact bytes do not match their claimed digest.
    #[error("guest component artifact digest does not match its content")]
    DigestMismatch,
    /// The artifact is not a valid component for the configured engine.
    #[error("could not compile the guest component: {0}")]
    Compile(#[source] wasmtime::Error),
}

/// Failure while applying one guest component generation.
#[derive(Debug, Error)]
pub enum ComponentApplyError {
    /// The component could not be instantiated against the minimal host.
    #[error("could not instantiate the guest component: {0}")]
    Instantiate(#[source] wasmtime::Error),
    /// The guest trapped while it was being activated.
    #[error("guest activation trapped: {0}")]
    ActivateTrap(#[source] wasmtime::Error),
    /// The guest rejected activation with its own diagnostic.
    #[error("guest activation rejected: {0}")]
    ActivateRejected(String),
    /// The guest component actor stopped before activation completed.
    #[error("the guest component actor stopped before activation")]
    ActorStopped,
    /// The guest trapped while its static manifest was being read.
    #[error("guest manifest trapped: {0}")]
    ManifestTrap(#[source] wasmtime::Error),
    /// A declared guest event subscription could not join the generation.
    #[error("could not register guest event subscription: {0}")]
    Subscription(#[from] ListenerRegistrationError),
    /// The generation no longer admitted the cleanup obligation.
    #[error("could not bind guest cleanup to the Cordis generation: {0}")]
    CleanupRegistration(#[from] EffectRegistrationError),
}

/// A cleanup diagnostic reported after the host has claimed guest disposal.
#[derive(Debug, Error)]
enum ComponentDisposeError {
    #[error("guest disposal trapped: {0}")]
    Trap(#[source] wasmtime::Error),
    #[error("guest disposal rejected: {0}")]
    Rejected(String),
    #[error("the guest component actor stopped before disposal")]
    ActorStopped,
}

#[derive(Debug, Error)]
enum ComponentHandleError {
    #[error("guest event handler trapped: {0}")]
    Trap(#[source] wasmtime::Error),
    #[error("guest event handler rejected: {0}")]
    Rejected(String),
    #[error("the guest component actor stopped before event handling")]
    ActorStopped,
}

pub(crate) struct HostState {
    context: Context,
    table: ResourceTable,
    wasi: WasiCtx,
    logger: Logger,
    configuration: Arc<[u8]>,
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

struct LiveComponent {
    store: Store<HostState>,
    bindings: CordisPlugin,
}

impl LiveComponent {
    fn arm_deadline(&mut self) {
        self.store.set_epoch_deadline(GUEST_DEADLINE_TICKS);
        self.store.epoch_deadline_trap();
    }

    async fn describe(
        &mut self,
    ) -> Result<bindings::exports::cordis::plugin::manifest::Descriptor, ComponentApplyError> {
        self.arm_deadline();
        self.store
            .run_concurrent(async |accessor| {
                self.bindings
                    .cordis_plugin_manifest()
                    .call_describe(accessor)
                    .await
            })
            .await
            .map_err(ComponentApplyError::ManifestTrap)?
            .map_err(ComponentApplyError::ManifestTrap)
    }

    async fn activate(&mut self) -> Result<(), ComponentApplyError> {
        self.arm_deadline();
        let outcome = self
            .store
            .run_concurrent(async |accessor| {
                self.bindings
                    .cordis_plugin_lifecycle()
                    .call_activate(accessor)
                    .await
            })
            .await
            .map_err(ComponentApplyError::ActivateTrap)?
            .map_err(ComponentApplyError::ActivateTrap)?;
        outcome.map_err(|error| ComponentApplyError::ActivateRejected(error.message))
    }

    async fn handle(
        &mut self,
        event: bindings::cordis::plugin::events::Event,
    ) -> Result<(), ComponentHandleError> {
        self.arm_deadline();
        let result = self
            .store
            .run_concurrent(async |accessor| {
                self.bindings
                    .cordis_plugin_event_handler()
                    .call_handle(accessor, event)
                    .await
            })
            .await
            .map_err(ComponentHandleError::Trap)?
            .map_err(ComponentHandleError::Trap)?;
        result.map_err(|error| ComponentHandleError::Rejected(error.message))
    }

    async fn dispose(&mut self) -> Result<(), ComponentDisposeError> {
        self.arm_deadline();
        let outcome = self
            .store
            .run_concurrent(async |accessor| {
                self.bindings
                    .cordis_plugin_lifecycle()
                    .call_dispose(accessor)
                    .await
            })
            .await
            .map_err(ComponentDisposeError::Trap)?
            .map_err(ComponentDisposeError::Trap)?;
        outcome.map_err(|error| ComponentDisposeError::Rejected(error.message))
    }
}

struct GenerationComponent {
    commands: mpsc::UnboundedSender<ComponentCommand>,
}

impl GenerationComponent {
    fn new(mut component: LiveComponent) -> Self {
        let (commands, mut receiver) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(command) = receiver.recv().await {
                match command {
                    ComponentCommand::Activate(response) => {
                        let _ = response.send(component.activate().await);
                    }
                    ComponentCommand::Dispose(response) => {
                        let _ = response.send(component.dispose().await);
                        break;
                    }
                    ComponentCommand::Handle { event, response } => {
                        let _ = response.send(
                            component
                                .handle(bindings::cordis::plugin::events::Event {
                                    name: event.name,
                                    payload: event.payload,
                                })
                                .await,
                        );
                    }
                }
            }
        });
        Self { commands }
    }

    async fn activate(&self) -> Result<(), ComponentApplyError> {
        let (send, receive) = oneshot::channel();
        self.commands
            .send(ComponentCommand::Activate(send))
            .map_err(|_| ComponentApplyError::ActorStopped)?;
        receive
            .await
            .map_err(|_| ComponentApplyError::ActorStopped)?
    }

    async fn handle(&self, event: HostEvent) -> Result<(), ComponentHandleError> {
        let (send, receive) = oneshot::channel();
        self.commands
            .send(ComponentCommand::Handle {
                event,
                response: send,
            })
            .map_err(|_| ComponentHandleError::ActorStopped)?;
        receive
            .await
            .map_err(|_| ComponentHandleError::ActorStopped)?
    }

    async fn dispose(&self) -> Result<(), ComponentDisposeError> {
        let (send, receive) = oneshot::channel();
        self.commands
            .send(ComponentCommand::Dispose(send))
            .map_err(|_| ComponentDisposeError::ActorStopped)?;
        receive
            .await
            .map_err(|_| ComponentDisposeError::ActorStopped)?
    }
}

enum ComponentCommand {
    Activate(oneshot::Sender<Result<(), ComponentApplyError>>),
    Dispose(oneshot::Sender<Result<(), ComponentDisposeError>>),
    Handle {
        event: HostEvent,
        response: oneshot::Sender<Result<(), ComponentHandleError>>,
    },
}

impl Plugin for ComponentPlugin {
    type Config = ComponentArtifact;
    type Input = ComponentInput;
    type PrepareError = ComponentPrepareError;
    type ApplyError = ComponentApplyError;

    fn prepare(
        &self,
        artifact: ComponentArtifact,
    ) -> Result<ComponentInput, ComponentPrepareError> {
        if Sha256::digest(&artifact.bytes).as_slice() != artifact.digest {
            return Err(ComponentPrepareError::DigestMismatch);
        }
        Component::new(&self.engine, &artifact.bytes)
            .map(|component| ComponentInput {
                component: Arc::new(component),
                configuration: artifact.configuration,
            })
            .map_err(ComponentPrepareError::Compile)
    }

    async fn apply(&self, ctx: Context, input: &ComponentInput) -> Result<(), ComponentApplyError> {
        let mut store = Store::new(
            &self.engine,
            HostState {
                context: ctx.clone(),
                table: ResourceTable::new(),
                wasi: WasiCtx::builder().build(),
                logger: ctx.logger().with_name("wasm-component"),
                configuration: input.configuration.clone(),
            },
        );
        store.set_epoch_deadline(INITIALIZATION_DEADLINE_TICKS);
        let bindings = CordisPlugin::instantiate_async(&mut store, &input.component, &self.linker)
            .await
            .map_err(ComponentApplyError::Instantiate)?;
        let mut live = LiveComponent { store, bindings };
        let descriptor = live.describe().await?;

        let generation = Arc::new(GenerationComponent::new(live));
        let cleanup_generation = generation.clone();
        if let Err(error) = ctx.effect(move || async move { cleanup_generation.dispose().await }) {
            let _ = generation.dispose().await;
            return Err(error.into());
        }

        generation.activate().await?;
        for name in descriptor.subscribed_events {
            let generation = generation.clone();
            ctx.on::<HostEvent, _>(observer(move |_, event: HostEvent| {
                let generation = generation.clone();
                let name = name.clone();
                async move {
                    if event.name == name {
                        generation.handle(event).await?;
                    }
                    Ok::<_, ComponentHandleError>(())
                }
            }))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{ComponentArtifact, ComponentPlugin, ComponentPrepareError};
    use cordis_core::Plugin;

    #[test]
    fn malformed_artifact_is_rejected_before_lifecycle_admission() {
        let plugin = ComponentPlugin::new().expect("the local Wasmtime engine is constructible");
        let artifact = ComponentArtifact::from_bytes(b"not a component".to_vec());

        assert!(plugin.prepare(artifact).is_err());
    }

    #[test]
    fn digest_mismatch_is_rejected_before_lifecycle_admission() {
        let plugin = ComponentPlugin::new().expect("the local Wasmtime engine is constructible");
        let artifact = ComponentArtifact::with_digest(b"not a component".to_vec(), [0; 32]);
        assert!(matches!(
            plugin.prepare(artifact),
            Err(ComponentPrepareError::DigestMismatch)
        ));
    }
}
