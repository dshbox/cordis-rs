//! Every auto trait the normative interface promises for `cordis-core`
//! (§Openness and auto traits). `Unpin` is never promised, so it is never
//! asserted here.

use cordis_core::{
    BoxError, Context, FiberHandle, FiberId, FiberState, QueryOutcome, UpdateOutcome,
    effect::{EffectFailure, EffectFailureKind, EffectRegistrationError, TaskRegistrationError},
    event::{
        DispatchError, DispatchOutcomeKind, EventOperation, InvocationFailure,
        InvocationFailureKind, ListenerRegistrationError, ListenerRegistrationId, ListenerRole,
        ParallelFailures,
    },
    lifecycle::{
        EraSwapError, EraSwapFailure, FiberRole, LifecycleOperation, LifecycleRecursion,
        PluginFailure, PluginFailureKind, ReadyError, RestartError, SpawnError, UpdateError,
        WaitStateError,
    },
    logger::BufferSizeZero,
    observation::{
        FiberSnapshot, ListenerChange, ObservationRouting, ResidencyChange, RuntimeObservation,
        RuntimeSnapshot, ScopeId, ServicePublicationId, ServiceSnapshot,
    },
    service::{
        ConfigResolutionError, RealmMappingError, ServiceControlError, ServiceLookupError,
        ServicePublishError, ServiceRealm,
    },
};

fn assert_send_sync<T: Send + Sync>() {}

/// Generic families are `Send + Sync` for every `Send + Sync` parameter.
fn generic_families<E: std::error::Error + Send + Sync, T: Send + Sync>() {
    assert_send_sync::<ConfigResolutionError<E>>();
    assert_send_sync::<QueryOutcome<T>>();
}

fn main() {
    // Runtime capabilities.
    assert_send_sync::<Context>();
    assert_send_sync::<FiberHandle>();

    // Errors and failures, with their kinds.
    assert_send_sync::<BoxError>();
    assert_send_sync::<RealmMappingError>();
    assert_send_sync::<ServiceLookupError>();
    assert_send_sync::<ServicePublishError>();
    assert_send_sync::<ServiceControlError>();
    let _ = generic_families::<std::io::Error, String>;
    assert_send_sync::<EffectRegistrationError>();
    assert_send_sync::<EffectFailure>();
    assert_send_sync::<EffectFailureKind>();
    assert_send_sync::<TaskRegistrationError>();
    assert_send_sync::<ListenerRegistrationError>();
    assert_send_sync::<DispatchError>();
    assert_send_sync::<InvocationFailure>();
    assert_send_sync::<InvocationFailureKind>();
    assert_send_sync::<ParallelFailures>();
    assert_send_sync::<SpawnError>();
    assert_send_sync::<ReadyError>();
    assert_send_sync::<RestartError>();
    assert_send_sync::<WaitStateError>();
    assert_send_sync::<UpdateError>();
    assert_send_sync::<EraSwapError>();
    assert_send_sync::<EraSwapFailure>();
    assert_send_sync::<PluginFailure>();
    assert_send_sync::<PluginFailureKind>();
    assert_send_sync::<LifecycleRecursion>();
    assert_send_sync::<LifecycleOperation>();
    assert_send_sync::<BufferSizeZero>();

    // Correlation identities.
    assert_send_sync::<FiberId>();
    assert_send_sync::<ServiceRealm>();
    assert_send_sync::<ServicePublicationId>();
    assert_send_sync::<ScopeId>();
    assert_send_sync::<ListenerRegistrationId>();

    // Snapshots.
    assert_send_sync::<RuntimeSnapshot>();
    assert_send_sync::<FiberSnapshot>();
    assert_send_sync::<ServiceSnapshot>();
    assert_send_sync::<FiberRole>();

    // Observation records and their vocabularies.
    assert_send_sync::<RuntimeObservation>();
    assert_send_sync::<ResidencyChange>();
    assert_send_sync::<ListenerChange>();
    assert_send_sync::<ObservationRouting>();
    assert_send_sync::<ListenerRole>();
    assert_send_sync::<EventOperation>();
    assert_send_sync::<DispatchOutcomeKind>();

    // Outcomes.
    assert_send_sync::<FiberState>();
    assert_send_sync::<UpdateOutcome>();
}
