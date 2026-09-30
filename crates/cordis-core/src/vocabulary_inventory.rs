//! In-crate exhaustive inventories of the public vocabularies that are
//! `#[non_exhaustive]` for downstream crates.
//!
//! Downstream matches must carry a wildcard arm, so the compiler no longer
//! guards the normative inventories there. Inside this crate
//! `#[non_exhaustive]` has no effect, so each `match` below fails to compile
//! when a variant is added or removed without a deliberate update here. Field
//! patterns use `..` on purpose: only the variant inventory is normative, and
//! growing a record variant's fields stays additive.

use crate::effect::EffectFailureKind;
use crate::event::{
    DispatchOutcomeKind, EventOperation, InvocationFailureKind, ListenerRole, Routing,
};
use crate::lifecycle::{LifecycleOperation, PluginFailureKind};
use crate::observation::{ObservationRouting, RuntimeObservation};

fn record_category(record: &RuntimeObservation) -> &'static str {
    match record {
        RuntimeObservation::FiberResidency { .. } => "FiberResidency",
        RuntimeObservation::FiberState { .. } => "FiberState",
        RuntimeObservation::ServiceVisibility { .. } => "ServiceVisibility",
        RuntimeObservation::ListenerRegistration { .. } => "ListenerRegistration",
        RuntimeObservation::DispatchCompleted { .. } => "DispatchCompleted",
    }
}

fn lifecycle_operation(value: LifecycleOperation) -> &'static str {
    match value {
        LifecycleOperation::Ready => "Ready",
        LifecycleOperation::WaitState => "WaitState",
        LifecycleOperation::Restart => "Restart",
        LifecycleOperation::Update => "Update",
        LifecycleOperation::EraSwap => "EraSwap",
        LifecycleOperation::Dispose => "Dispose",
        LifecycleOperation::RemovePlugins => "RemovePlugins",
    }
}

fn plugin_failure_kind(value: PluginFailureKind) -> &'static str {
    match value {
        PluginFailureKind::ReturnedError => "ReturnedError",
        PluginFailureKind::Panic => "Panic",
    }
}

fn effect_failure_kind(value: EffectFailureKind) -> &'static str {
    match value {
        EffectFailureKind::ReturnedError => "ReturnedError",
        EffectFailureKind::Panic => "Panic",
    }
}

fn invocation_failure_kind(value: InvocationFailureKind) -> &'static str {
    match value {
        InvocationFailureKind::ReturnedError => "ReturnedError",
        InvocationFailureKind::Panic => "Panic",
    }
}

fn dispatch_outcome_kind(value: DispatchOutcomeKind) -> &'static str {
    match value {
        DispatchOutcomeKind::Completed => "Completed",
        DispatchOutcomeKind::Answered => "Answered",
        DispatchOutcomeKind::Missed => "Missed",
        DispatchOutcomeKind::Failed => "Failed",
    }
}

fn event_operation(value: EventOperation) -> &'static str {
    match value {
        EventOperation::Emit => "Emit",
        EventOperation::EmitParallel => "EmitParallel",
        EventOperation::Query => "Query",
        EventOperation::Waterfall => "Waterfall",
    }
}

fn listener_role(value: ListenerRole) -> &'static str {
    match value {
        ListenerRole::Observer => "Observer",
        ListenerRole::Responder => "Responder",
        ListenerRole::Mapper => "Mapper",
        ListenerRole::Around => "Around",
    }
}

fn routing(value: &Routing) -> &'static str {
    match value {
        Routing::Unscoped => "Unscoped",
        Routing::Scoped(_) => "Scoped",
    }
}

fn observation_routing(value: &ObservationRouting) -> &'static str {
    match value {
        ObservationRouting::Unscoped => "Unscoped",
        ObservationRouting::Scoped(_) => "Scoped",
    }
}

#[test]
fn runtime_observation_has_exactly_five_record_categories() {
    // ADR 0035: "exactly five immutable record categories". The exhaustive
    // match in `record_category` is the compiler guard; a constructible
    // record pins that its name is one of the five.
    const CATEGORIES: [&str; 5] = [
        "FiberResidency",
        "FiberState",
        "ServiceVisibility",
        "ListenerRegistration",
        "DispatchCompleted",
    ];
    let record = RuntimeObservation::DispatchCompleted {
        operation: EventOperation::Emit,
        event: "inventory",
        routing: ObservationRouting::Unscoped,
        outcome: DispatchOutcomeKind::Completed,
    };
    assert!(CATEGORIES.contains(&record_category(&record)));
}

#[test]
fn event_vocabularies_keep_their_inventories() {
    assert_eq!(
        [
            EventOperation::Emit,
            EventOperation::EmitParallel,
            EventOperation::Query,
            EventOperation::Waterfall,
        ]
        .map(event_operation),
        ["Emit", "EmitParallel", "Query", "Waterfall"]
    );
    assert_eq!(
        [
            ListenerRole::Observer,
            ListenerRole::Responder,
            ListenerRole::Mapper,
            ListenerRole::Around,
        ]
        .map(listener_role),
        ["Observer", "Responder", "Mapper", "Around"]
    );
    assert_eq!(
        [
            DispatchOutcomeKind::Completed,
            DispatchOutcomeKind::Answered,
            DispatchOutcomeKind::Missed,
            DispatchOutcomeKind::Failed,
        ]
        .map(dispatch_outcome_kind),
        ["Completed", "Answered", "Missed", "Failed"]
    );

    // `ObservationRouting` is the one-to-one projection of `Routing`.
    let scoped = Routing::Scoped(crate::Context::new().scope());
    for (route, name) in [(Routing::Unscoped, "Unscoped"), (scoped, "Scoped")] {
        assert_eq!(routing(&route), name);
        assert_eq!(
            observation_routing(&ObservationRouting::from_routing(&route)),
            name
        );
    }
}

#[test]
fn lifecycle_operation_covers_every_refused_self_wait() {
    // ADR 0029: the recursion refusal "covers ready, wait_state, restart,
    // update, era swap, dispose, and typed group removal".
    assert_eq!(
        [
            LifecycleOperation::Ready,
            LifecycleOperation::WaitState,
            LifecycleOperation::Restart,
            LifecycleOperation::Update,
            LifecycleOperation::EraSwap,
            LifecycleOperation::Dispose,
            LifecycleOperation::RemovePlugins,
        ]
        .map(lifecycle_operation),
        [
            "Ready",
            "WaitState",
            "Restart",
            "Update",
            "EraSwap",
            "Dispose",
            "RemovePlugins"
        ]
    );
}

#[test]
fn failure_kinds_keep_returned_error_and_panic() {
    assert_eq!(
        [PluginFailureKind::ReturnedError, PluginFailureKind::Panic].map(plugin_failure_kind),
        ["ReturnedError", "Panic"]
    );
    assert_eq!(
        [EffectFailureKind::ReturnedError, EffectFailureKind::Panic].map(effect_failure_kind),
        ["ReturnedError", "Panic"]
    );
    assert_eq!(
        [
            InvocationFailureKind::ReturnedError,
            InvocationFailureKind::Panic,
        ]
        .map(invocation_failure_kind),
        ["ReturnedError", "Panic"]
    );
}
