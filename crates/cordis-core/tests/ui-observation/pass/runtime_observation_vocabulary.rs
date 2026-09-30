use cordis_core::event::{DispatchOutcomeKind, EventOperation, ListenerOptions};
use cordis_core::observation::{ListenerChange, ObservationRouting, ResidencyChange, RuntimeObservation, ScopeId};
use std::{fmt::Debug, hash::Hash};
fn small<T: Clone + Copy + Debug + Eq>(_: T) {}
fn opaque<T: Clone + Debug + Eq + Hash>(_: &T) {}
fn opaque_type<T: Clone + Debug + Eq + Hash>() {}
fn clone_debug<T: Clone + Debug>() {}
fn inspect(observation: &RuntimeObservation) { match observation {
RuntimeObservation::FiberResidency { change, fiber, .. } => { small(*change); let _ = fiber.id(); }
RuntimeObservation::FiberState { fiber, previous, current, .. } => { opaque(fiber); let _ = (*previous, *current); }
RuntimeObservation::ServiceVisibility { service, realm, previous, current, .. } => { let _ = (service, realm, previous, current); }
RuntimeObservation::ListenerRegistration { change, listener, event, role, scope, options, .. } => { small(*change); opaque(listener); let _ = event; small(*role); opaque(scope); let _: ListenerOptions = *options; }
RuntimeObservation::DispatchCompleted { operation, event, routing, outcome, .. } => { small(*operation); let _ = event; let _: &ObservationRouting = routing; small(*outcome); }
_ => {}
} }
fn main() { small(ResidencyChange::Admitted); small(ResidencyChange::Removed); small(ListenerChange::Registered); small(ListenerChange::Unregistered); small(EventOperation::Emit); small(EventOperation::EmitParallel); small(EventOperation::Query); small(EventOperation::Waterfall); small(DispatchOutcomeKind::Completed); small(DispatchOutcomeKind::Answered); small(DispatchOutcomeKind::Missed); small(DispatchOutcomeKind::Failed); opaque_type::<ScopeId>(); clone_debug::<RuntimeObservation>(); let _ = inspect; }
