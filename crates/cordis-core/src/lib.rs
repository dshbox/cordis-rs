//! # cordis-core
//!
//! The consumer-agnostic runtime foundation ported from upstream cordis
//! (TypeScript): typed event dispatch, effects, fibers and settlement,
//! scopes/isolate derivation, and the service/notification machinery that
//! drives them — over locked storage with snapshot-only reads (ADR 0010).
//!
//! ## Module map
//!
//! | upstream | cordis-core | notes |
//! |---|---|---|
//! | `events.ts` | [`event`] | named typed contracts, semantic listener roles, explicit `Routing` |
//! | `utils.ts` (disposables) | [`effect`] | LIFO cleanup lists, sync + async registration |
//! | `context.ts` | [`Context`] | cheap-clone Runtime view over orthogonal axes |
//! | Plugin preparation | [`plugin`] | typed preparation, sealing, and normalized dependency declarations |
//! | `fiber.ts` | [`Fork`] and lifecycle types | private settle protocol, semantic public facade |
//! | `registry.ts` | private Registry implementation | Runtime residency machinery |
//! | `logger.ts` | [`logger`] | named channels, [`logger::Exporter`] fan-out, level routing |
//! | `reflect.ts` + `service.ts` | [`service`] | exact Service-slot publication, lookup, and dependency visibility |
//!
//! The dependency contract is fixed from day one (ADR 0002): no serde,
//! no serde_json, no tokio `time` — timer discipline lives in
//! cordis-timer, loader config in cordis-loader.
//!
//! [`Context::run`]: crate::Context::run

mod context;
pub mod effect;
mod events;
mod fiber;
pub mod logger;
pub mod observation;
pub mod plugin;
mod registry;
pub mod service;

mod contained;
mod deadline;
mod deps;
mod gated;
mod update;

/// Workspace-only implementation seams for semantic leaf crates.
///
/// This module exists only when the non-default `internal-api` Cargo feature is
/// enabled. It is not a supported downstream interface and is deliberately not
/// re-exported by the application facade.
#[cfg(feature = "internal-api")]
#[doc(hidden)]
pub mod __internal {
    use crate::Context;

    /// Probe whether the selected Context generation currently admits cleanup.
    ///
    /// A later cleanup registration remains authoritative and may still lose a
    /// race with generation closure.
    pub fn generation_cleanup_admitted(ctx: &Context) -> bool {
        ctx.fiber().assert_can_register().is_ok()
    }
}

/// Fiber identity, lifecycle control, and creation outcomes.
pub mod lifecycle {
    pub use crate::fiber::{
        EraSwapError, EraSwapFailure, FiberId, FiberRole, FiberState, Fork, LifecycleOperation,
        LifecycleRecursion, PluginFailure, PluginFailureKind, ReadyError, RestartError, SpawnError,
        UpdateError, UpdateOutcome, WaitStateError,
    };
    pub use crate::update::{UpdateListener, UpdateNext};
}

pub use context::Context;
/// Optional application-selected erasure convenience; framework failures use operation-specific families.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
/// Typed Event contracts, routing, listener roles, and dispatch errors.
pub mod event {
    pub use crate::events::{
        DispatchError, DispatchOutcomeKind, Event, EventOperation, InvocationFailure,
        InvocationFailureKind, Listener, ListenerOptions, ListenerRegistration,
        ListenerRegistrationError, ListenerRegistrationId, ListenerRole, Next, ParallelFailures,
        QueryOutcome, Routing, Scope, StatefulCallback, around, mapper, mapper_sync, observer,
        observer_sync, responder, responder_sync, with_state,
    };
}

pub use event::{Event, QueryOutcome, Routing, Scope};
pub use fiber::{FiberId, FiberState, Fork, UpdateOutcome};
pub use logger::{Level, Logger};
pub use plugin::{InjectSpec, Plugin, PreparedChange, PreparedPlugin};
pub use service::{ConfigurableService, Service, ServiceRealm};
