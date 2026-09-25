//! Typed Event contracts and semantic dispatch vocabulary.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// A Runtime-local named typed Event contract.
///
/// The marker itself has no supertraits. The Runtime binds `NAME` to one
/// compatible `Args`/`Output` pair; Rust type identity never routes delivery.
pub trait Event {
    /// Runtime-local semantic Event name.
    const NAME: &'static str;
    /// Owned input carried by one dispatch invocation.
    type Args: Send + 'static;
    /// Owned semantic output produced by answering or waterfall roles.
    type Output: Send + 'static;
}

/// Explicit Event routing for every dispatch operation.
#[derive(Clone, Debug)]
pub enum Routing {
    /// Consider registrations from every Scope.
    Unscoped,
    /// Consider globals plus registrations whose Scope is ancestor-or-self.
    Scoped(crate::context::Scope),
}

/// Sequential query result with explicit answer presence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryOutcome<T> {
    /// No Responder produced an answer.
    Miss,
    /// The first Responder answer.
    Answer(T),
}

/// Opaque Runtime-local identity of one exact listener registration occurrence.
///
/// The identity is correlation-only: it grants no registration or removal authority.
#[derive(Clone)]
pub struct ListenerRegistrationId(Arc<u8>);

pub(crate) fn fresh_listener_registration_id() -> ListenerRegistrationId {
    ListenerRegistrationId(Arc::new(0))
}

impl PartialEq for ListenerRegistrationId {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for ListenerRegistrationId {}

impl Hash for ListenerRegistrationId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self.0), state);
    }
}

impl fmt::Debug for ListenerRegistrationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ListenerRegistrationId(..)")
    }
}

/// Semantic listener role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenerRole {
    /// Notification participant; contributes no answer.
    Observer,
    /// Notification/query participant that may answer a query.
    Responder,
    /// Waterfall forward-transform participant.
    Mapper,
    /// Waterfall onion participant with a consuming [`crate::event::Next`].
    Around,
}

/// Event operation used by role preflight diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventOperation {
    /// Ordered awaited notification.
    Emit,
    /// Parallel notification operation.
    EmitParallel,
    /// Sequential first-answer query.
    Query,
    /// Owned Mapper/Around waterfall.
    Waterfall,
}

/// Semantic completion kind reported by Runtime observation.
///
/// This enum carries no display, wire-name, ordering, or serialization contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchOutcomeKind {
    /// A notification or waterfall primitive completed successfully.
    Completed,
    /// A query completed with an answer.
    Answered,
    /// A query completed without an answer.
    Missed,
    /// The primitive completed with a dispatch failure.
    Failed,
}

/// Why one claimed listener invocation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvocationFailureKind {
    /// The user callback or tail returned an error.
    ReturnedError,
    /// A state factory, callback, or awaited callback future panicked.
    Panic,
}

/// Opaque normalized listener invocation failure.
#[derive(Debug)]
pub struct InvocationFailure {
    pub(crate) registration: Option<ListenerRegistrationId>,
    pub(crate) kind: InvocationFailureKind,
    pub(crate) diagnostic: String,
}

impl InvocationFailure {
    /// Return the exact listener occurrence correlated with this failure.
    ///
    /// Framework tail execution has no listener occurrence and therefore returns `None`.
    pub fn registration_id(&self) -> Option<&ListenerRegistrationId> {
        self.registration.as_ref()
    }
    /// Return the normalized failure kind.
    pub fn kind(&self) -> InvocationFailureKind {
        self.kind
    }
    /// Return the owned failure diagnostic text.
    pub fn diagnostic(&self) -> &str {
        &self.diagnostic
    }
    pub(crate) fn returned(diagnostic: String) -> Self {
        Self {
            registration: None,
            kind: InvocationFailureKind::ReturnedError,
            diagnostic,
        }
    }
    pub(crate) fn panic(diagnostic: String) -> Self {
        Self {
            registration: None,
            kind: InvocationFailureKind::Panic,
            diagnostic,
        }
    }
    pub(crate) fn correlate(mut self, registration: ListenerRegistrationId) -> Self {
        if self.registration.is_none() {
            self.registration = Some(registration);
        }
        self
    }
}

/// Opaque non-empty parallel-notification failure collection.
#[derive(Debug)]
pub struct ParallelFailures {
    failures: Vec<InvocationFailure>,
}

impl ParallelFailures {
    /// Return the failures in effective listener order.
    pub fn failures(&self) -> &[InvocationFailure] {
        &self.failures
    }

    pub(crate) fn new(failures: Vec<InvocationFailure>) -> Self {
        assert!(
            !failures.is_empty(),
            "ParallelFailures is constructed only for a non-empty failure set"
        );
        Self { failures }
    }
}

impl fmt::Display for InvocationFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.diagnostic)
    }
}
impl std::error::Error for InvocationFailure {}

/// Dispatch preflight or invocation failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DispatchError {
    #[error("event `{event}` is already bound to an incompatible contract")]
    /// The name is already bound to another `Args`/`Output` contract.
    EventContractMismatch {
        /// Conflicting Runtime-local Event name.
        event: &'static str,
    },
    #[error("routing Scope belongs to another Runtime")]
    /// The supplied Scope belongs to another Runtime.
    ForeignScope,
    #[error("{role:?} listener is incompatible with {operation:?}")]
    /// A selected registration has a role incompatible with this operation.
    IncompatibleRole {
        /// Dispatch operation being attempted.
        operation: EventOperation,
        /// Incompatible selected listener role.
        role: ListenerRole,
    },
    #[error("listener invocation failed: {0}")]
    /// One claimed invocation failed after preflight.
    Invocation(InvocationFailure),
    #[error("parallel event dispatch failed")]
    /// One or more claimed parallel-notification invocations failed.
    Parallel(ParallelFailures),
}

/// Listener registration failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ListenerRegistrationError {
    #[error("cannot register listener on an inactive Context")]
    /// The registering Context's current Fiber generation is closed.
    InactiveContext,
    #[error("event `{event}` is already bound to an incompatible contract")]
    /// The name is already bound to another `Args`/`Output` contract.
    EventContractMismatch {
        /// Conflicting Runtime-local Event name.
        event: &'static str,
    },
}

#[doc(hidden)]
pub type ErasedPayload = Box<dyn std::any::Any + Send>;

pub(crate) fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    crate::contained::consume_panic_payload(payload, "listener panicked")
}
