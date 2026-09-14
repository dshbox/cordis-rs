#![allow(unused_imports)]
use cordis_core::{
    BoxError, ConfigurableService, Context, Event, FiberId, FiberState, FiberHandle, InjectSpec, Level,
    Logger, Plugin, PreparedChange, PreparedPlugin, QueryOutcome, Routing, Scope, Service,
    ServiceRealm, UpdateOutcome,
};
use cordis_core::plugin::{Plugin as ModulePlugin, PreparedPlugin as ModulePreparedPlugin, PreparedChange as ModulePreparedChange, InjectSpec as ModuleInjectSpec};
use cordis_core::lifecycle::{EraSwapError, EraSwapFailure, FiberRole, LifecycleOperation, LifecycleRecursion, PluginFailure, PluginFailureKind, ReadyError, RestartError, SpawnError, UpdateError, UpdateListener, UpdateNext, WaitStateError};
use cordis_core::service::{ServicePublication, RealmMappingError, ServiceLookupError, ServicePublishError, ServiceControlError, ConfigResolutionError};
use cordis_core::event::{DispatchError, DispatchOutcomeKind, EventOperation, InvocationFailure, InvocationFailureKind, Listener, ListenerOptions, ListenerRegistration, ListenerRegistrationError, ListenerRegistrationId, ListenerRole, Next, ParallelFailures, StatefulCallback, observer, observer_sync, responder, responder_sync, mapper, mapper_sync, around, with_state};
use cordis_core::effect::{CleanupResult, EffectRegistration, EffectRegistrationError, EffectFailure, EffectFailureKind, TaskRegistrationError};
use cordis_core::logger::{LogRecord, Exporter, ExporterRegistration, BufferExporter, BufferSizeZero};
use cordis_core::observation::{RuntimeSnapshot, FiberSnapshot, ServiceSnapshot, ServicePublicationId, ScopeId, ObservationRouting, RuntimeObservation, ResidencyChange, ListenerChange, RuntimeObserver};
fn main() {
    let _ = std::any::TypeId::of::<Context>();
    let _ = std::any::TypeId::of::<BoxError>();
}
