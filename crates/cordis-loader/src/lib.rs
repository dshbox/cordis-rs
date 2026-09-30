//! Declarative loading for Cordis.
//!
//! This crate exposes immutable validated Loader plans, the synchronous target-
//! resolution firewall, and partial execution with one complete ordered semantic
//! outcome per plan entry. Source declarations are frozen without exposing
//! backing topology; resolver adaptation ends raw JSON before lifecycle admission.
//! Per-execution realm interpretation and pre-delivery result handoff remain private
//! Loader protocol layers.

mod handoff;
#[cfg(test)]
mod vocabulary_inventory;

/// Complete per-entry execution outcomes and Loader failures.
pub mod outcome;
/// Immutable plan construction and serialized Loader source schema.
pub mod plan;
/// Synchronous target-specific resolution and typed JSON preparation.
pub mod resolver;

pub use outcome::LoadOutcome;
pub use plan::{EntryGroup, EntryId, LoadPlan, LoadPlanBuilder, PluginEntry};
pub use resolver::PluginResolver;
