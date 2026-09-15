use cordis_core::{
    BoxError,
    effect::{EffectFailure, EffectFailureKind, EffectRegistrationError, TaskRegistrationError},
    event::{DispatchError, InvocationFailure, InvocationFailureKind, ListenerRegistrationError, ParallelFailures},
    lifecycle::{EraSwapError, EraSwapFailure, LifecycleOperation, LifecycleRecursion, PluginFailure, PluginFailureKind, ReadyError, RestartError, SpawnError, UpdateError, WaitStateError},
    service::{ConfigResolutionError, RealmMappingError, ServiceControlError, ServiceLookupError, ServicePublishError},
};

fn realm(e: RealmMappingError) { match e { RealmMappingError::DuplicateService { .. } | RealmMappingError::ForeignRealm { .. } => {}, _ => {} } }
fn lookup(e: ServiceLookupError) { match e { ServiceLookupError::Unavailable { .. } | ServiceLookupError::ContractMismatch { .. } => {}, _ => {} } }
fn publish(e: ServicePublishError) { match e { ServicePublishError::InactiveContext | ServicePublishError::DuplicatePublication { .. } | ServicePublishError::ContractMismatch { .. } => {}, _ => {} } }
fn control(e: ServiceControlError) { match e { ServiceControlError::StalePublication { .. } | ServiceControlError::MutationClosed { .. } => {}, _ => {} } }
fn config(e: ConfigResolutionError<std::io::Error>) { match e { ConfigResolutionError::ContractMismatch { .. } => {}, ConfigResolutionError::Compose(inner) => { let _: std::io::Error = inner; }, _ => {} } }
fn effect_registration(e: EffectRegistrationError) { match e { EffectRegistrationError::InactiveContext => {}, _ => {} } }
fn task_registration(e: TaskRegistrationError) { match e { TaskRegistrationError::InactiveContext | TaskRegistrationError::ExecutorUnavailable => {}, _ => {} } }
fn listener_registration(e: ListenerRegistrationError) { match e { ListenerRegistrationError::InactiveContext | ListenerRegistrationError::EventContractMismatch { .. } => {}, _ => {} } }
fn dispatch(e: DispatchError) { match e { DispatchError::EventContractMismatch { .. } | DispatchError::ForeignScope | DispatchError::IncompatibleRole { .. } | DispatchError::Invocation(_) | DispatchError::Parallel(_) => {}, _ => {} } }
fn spawn(e: SpawnError) { match e { SpawnError::InactiveContext | SpawnError::InitialApply(_) | SpawnError::Interrupted => {}, _ => {} } }
fn ready(e: ReadyError) { match e { ReadyError::Recursion(_) | ReadyError::Apply(_) => {}, _ => {} } }
fn restart(e: RestartError) { match e { RestartError::Closed | RestartError::Recursion(_) | RestartError::Apply(_) => {}, _ => {} } }
fn wait(e: WaitStateError) { match e { WaitStateError::Elapsed | WaitStateError::Recursion(_) => {}, _ => {} } }
fn update(e: UpdateError) { match e { UpdateError::PluginContractMismatch | UpdateError::Closed | UpdateError::Recursion(_) | UpdateError::Control(_) | UpdateError::AdmissionLost | UpdateError::Apply(_) => {}, _ => {} } }
fn era_failure(e: EraSwapFailure) { match e { EraSwapFailure::SuccessorApply(_) | EraSwapFailure::SuccessorLost => {}, _ => {} } }
fn era(e: EraSwapError) { match e { EraSwapError::PluginContractMismatch | EraSwapError::Closed | EraSwapError::Recursion(_) | EraSwapError::Incomplete(_) => {}, _ => {} } }
fn plugin_failure(e: &PluginFailure) { let _: PluginFailureKind = e.kind(); let _: &str = e.diagnostic(); }
fn effect_failure(e: &EffectFailure) { let _: EffectFailureKind = e.kind(); let _: &str = e.diagnostic(); }
fn invocation_failure(e: &InvocationFailure) { let _: InvocationFailureKind = e.kind(); let _: &str = e.diagnostic(); let _ = e.registration_id(); }
fn recursion(e: &LifecycleRecursion) { let _: LifecycleOperation = e.operation(); let _ = e.fiber_id(); }
fn parallel(e: &ParallelFailures) { let _: &[InvocationFailure] = e.failures(); }
fn application_erasure(_: Option<BoxError>) {}

fn main() {
    let _ = (realm, lookup, publish, control, config, effect_registration, task_registration,
        listener_registration, dispatch, spawn, ready, restart, wait, update, era_failure, era,
        plugin_failure, effect_failure, invocation_failure, recursion, parallel, application_erasure);
}
