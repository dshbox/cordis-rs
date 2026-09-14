//! Application-facing entry point for the Cordis v3 runtime.
//!
//! `cordis-rs` preserves the historical `cordis` import name while the
//! canonical runtime contract lives in `cordis-core`. Framework and plugin
//! authors may depend on `cordis-core` directly; applications can depend on
//! this facade to keep `use cordis::...` imports.

pub use cordis_core::{
    BoxError, ConfigurableService, Context, Event, FiberHandle, FiberId, FiberState, InjectSpec,
    Level, Logger, Plugin, PreparedChange, PreparedPlugin, QueryOutcome, Routing, Scope, Service,
    ServiceRealm, UpdateOutcome, effect, event, lifecycle, logger, observation, plugin, service,
};
