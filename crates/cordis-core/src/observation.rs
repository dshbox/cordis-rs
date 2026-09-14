//! Read-only Runtime snapshots and immutable committed transition records.
//!
//! A snapshot is a flat current-state projection, not Registry topology, a
//! globally linearizable instant, durable history, replay, lookup, or control.
//! The two collections are unordered and need not be referentially closed.

use crate::event::{
    DispatchOutcomeKind, EventOperation, ListenerOptions, ListenerRegistrationId, ListenerRole,
    Routing,
};
use crate::fiber::{FiberId, FiberRole, FiberState};
use crate::service::{ServiceOccurrenceId, ServiceRealm};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Opaque Runtime-local correlation identity for one Service publication.
#[derive(Clone)]
pub struct ServicePublicationId(pub(crate) ServiceOccurrenceId);

impl PartialEq for ServicePublicationId {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for ServicePublicationId {}
impl Hash for ServicePublicationId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}
impl std::fmt::Debug for ServicePublicationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ServicePublicationId(..)")
    }
}

/// Opaque Runtime-local correlation identity for one Event Scope position.
#[derive(Clone)]
pub struct ScopeId {
    membership: Arc<crate::context::ScopeMembership>,
    layer: Option<Arc<crate::context::ScopeNode>>,
}

impl ScopeId {
    pub(crate) fn from_scope(scope: &crate::context::Scope) -> Self {
        Self {
            membership: scope.membership.clone(),
            layer: scope.layer.clone(),
        }
    }
    pub(crate) fn from_context(ctx: &crate::Context) -> Self {
        Self {
            membership: ctx.root.scope_membership.clone(),
            layer: ctx.scope.clone(),
        }
    }
    pub(crate) fn from_layer(
        membership: &Arc<crate::context::ScopeMembership>,
        layer: Option<Arc<crate::context::ScopeNode>>,
    ) -> Self {
        Self {
            membership: membership.clone(),
            layer,
        }
    }
}
impl PartialEq for ScopeId {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.membership, &other.membership)
            && match (&self.layer, &other.layer) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
    }
}
impl Eq for ScopeId {}
impl Hash for ScopeId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self.membership), state);
        match &self.layer {
            Some(layer) => std::ptr::hash(Arc::as_ptr(layer), state),
            None => 0usize.hash(state),
        }
    }
}
impl std::fmt::Debug for ScopeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ScopeId(..)")
    }
}

/// Correlation-only projection of one dispatch's routing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservationRouting {
    /// Every Scope is eligible.
    Unscoped,
    /// Eligibility is rooted at one opaque Scope position.
    Scoped(ScopeId),
}
impl ObservationRouting {
    pub(crate) fn from_routing(routing: &Routing) -> Self {
        match routing {
            Routing::Unscoped => Self::Unscoped,
            Routing::Scoped(scope) => Self::Scoped(ScopeId::from_scope(scope)),
        }
    }
}

/// Fiber residency transition kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResidencyChange {
    /// One ordinary Fiber became Registry-resident.
    Admitted,
    /// One ordinary Fiber completed exact Registry unlink.
    Removed,
}

/// Listener occurrence transition kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenerChange {
    /// One exact listener occurrence committed registration.
    Registered,
    /// One exact listener occurrence committed unregistration.
    Unregistered,
}

/// One immutable committed Runtime transition record.
#[derive(Debug, Clone)]
pub enum RuntimeObservation {
    /// One ordinary Fiber entered or left Runtime residency.
    FiberResidency {
        /// Whether the occurrence was admitted or removed.
        change: ResidencyChange,
        /// Self-contained Fiber facts captured at the residency boundary.
        fiber: FiberSnapshot,
    },
    /// One Fiber lifecycle state transitioned.
    FiberState {
        /// Opaque Fiber correlation identity.
        fiber: FiberId,
        /// State before the committed transition.
        previous: FiberState,
        /// State after the committed transition.
        current: FiberState,
    },
    /// Visibility of one exact Service slot changed.
    ServiceVisibility {
        /// Service contract name.
        service: String,
        /// Exact Runtime-local Service realm.
        realm: ServiceRealm,
        /// Previously visible exact publication, if any.
        previous: Option<ServicePublicationId>,
        /// Currently visible exact publication, if any.
        current: Option<ServicePublicationId>,
    },
    /// One exact Event listener occurrence registered or unregistered.
    ListenerRegistration {
        /// Registration or unregistration transition.
        change: ListenerChange,
        /// Opaque exact listener occurrence identity.
        listener: ListenerRegistrationId,
        /// Semantic Event name.
        event: &'static str,
        /// Semantic listener role.
        role: ListenerRole,
        /// Opaque registration Scope correlation identity.
        scope: ScopeId,
        /// Immutable registration options.
        options: ListenerOptions,
    },
    /// One primitive Event dispatch completed.
    DispatchCompleted {
        /// Primitive dispatch operation.
        operation: EventOperation,
        /// Semantic Event name.
        event: &'static str,
        /// Correlation-only routing projection.
        routing: ObservationRouting,
        /// Semantic completion outcome.
        outcome: DispatchOutcomeKind,
    },
}

pub(crate) type ObservationCallback = Arc<
    dyn Fn(
            crate::Context,
            RuntimeObservation,
        ) -> crate::effect::BoxFuture<std::result::Result<(), String>>
        + Send
        + Sync,
>;
type RegisteredObserver = Arc<
    dyn Fn(RuntimeObservation) -> crate::effect::BoxFuture<std::result::Result<(), String>>
        + Send
        + Sync,
>;

pub(crate) mod runtime_observer_sealed {
    use super::ObservationCallback;

    pub trait Sealed: Send + Sync + 'static {
        fn into_callback(self) -> ObservationCallback;
    }
}

/// Sealed capability accepted by [`crate::Context::observe_runtime`].
///
/// Runtime observers receive the registering [`crate::Context`] for attribution
/// and one immutable committed [`RuntimeObservation`]. Delivery is detached,
/// Runtime-wide, unscoped, parallel, attempt-all, and best-effort.
pub trait RuntimeObserver: runtime_observer_sealed::Sealed {}
impl<T> RuntimeObserver for T where T: runtime_observer_sealed::Sealed {}

pub(crate) struct ObservationHub {
    /// Private cleanup-key space only; never a delivery-order authority.
    next_id: AtomicU64,
    observers: parking_lot::Mutex<HashMap<u64, RegisteredObserver>>,
    #[cfg(test)]
    records: parking_lot::Mutex<Vec<RuntimeObservation>>,
}

impl Default for ObservationHub {
    fn default() -> Self {
        Self {
            next_id: AtomicU64::new(0),
            observers: parking_lot::Mutex::new(HashMap::new()),
            #[cfg(test)]
            records: parking_lot::Mutex::new(Vec::new()),
        }
    }
}

struct ObserverPublish<'a> {
    hub: &'a ObservationHub,
    id: u64,
    callback: Option<RegisteredObserver>,
}
impl crate::gated::PublishStep for ObserverPublish<'_> {
    fn publish(&mut self) -> std::result::Result<(), crate::gated::PublishRefused> {
        let mut observers = self.hub.observers.lock();
        match observers.entry(self.id) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(self.callback.take().expect("observer publish runs once"));
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                unreachable!("observer identities are fresh and never reused")
            }
        }
        Ok(())
    }
}

impl ObservationHub {
    pub(crate) fn register<O>(
        self: &Arc<Self>,
        ctx: &crate::Context,
        observer: O,
    ) -> std::result::Result<(), crate::effect::EffectRegistrationError>
    where
        O: RuntimeObserver,
    {
        let id = self
            .next_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .expect("Runtime observer identity space exhausted")
            + 1;
        let callback = runtime_observer_sealed::Sealed::into_callback(observer);
        let registration = ctx.clone();
        let callback: RegisteredObserver =
            Arc::new(move |record| callback(registration.clone(), record));
        let hub = self.clone();
        let cleanup = crate::effect::sync_cleanup(move || {
            hub.remove(id);
        });
        let mut publish = ObserverPublish {
            hub: self,
            id,
            callback: Some(callback),
        };
        crate::gated::push_gated(ctx.fiber(), cleanup, &mut publish)
            .map_err(|_| crate::effect::EffectRegistrationError::InactiveContext)?;
        Ok(())
    }

    fn remove(&self, id: u64) {
        let removed = { self.observers.lock().remove(&id) };
        // The callback may be the last Arc retaining arbitrary observer captures.
        // Destroy it only after observer bookkeeping synchronization is released.
        drop(removed);
    }

    pub(crate) fn publish(&self, record: RuntimeObservation) {
        #[cfg(test)]
        self.records.lock().push(record.clone());

        let observers = self.observers.lock().values().cloned().collect::<Vec<_>>();
        if observers.is_empty() {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        for callback in observers {
            let record = record.clone();
            // The callback context was captured by the registration wrapper;
            // publication owns no source-operation completion dependency.
            handle.spawn(async move {
                let outcome =
                    crate::contained::catch_contained(async move { callback(record).await }).await;
                match outcome {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => crate::contained::report_text(
                        None,
                        format!("cordis: runtime observer failed: {error}"),
                    ),
                    Err(payload) => crate::contained::report_text(
                        None,
                        format!(
                            "cordis: runtime observer panicked: {}",
                            crate::contained::payload_text(&payload)
                        ),
                    ),
                }
            });
        }
    }

    #[cfg(test)]
    pub(crate) fn take(&self) -> Vec<RuntimeObservation> {
        std::mem::take(&mut *self.records.lock())
    }
}

/// A flat read-only projection of one Runtime's current semantic state.
#[derive(Debug, Clone)]
pub struct RuntimeSnapshot {
    pub(crate) fibers: Vec<FiberSnapshot>,
    pub(crate) services: Vec<ServiceSnapshot>,
}

impl RuntimeSnapshot {
    /// Current root plus Registry-resident ordinary Fibers, in no semantic order.
    pub fn fibers(&self) -> &[FiberSnapshot] {
        &self.fibers
    }
    /// Current occupied Service publication occurrences, in no semantic order.
    pub fn services(&self) -> &[ServiceSnapshot] {
        &self.services
    }
}

/// One self-consistent Fiber record in a Runtime snapshot.
#[derive(Debug, Clone)]
pub struct FiberSnapshot {
    pub(crate) id: FiberId,
    pub(crate) role: FiberRole,
    pub(crate) name: String,
    pub(crate) state: FiberState,
    pub(crate) missing_services: Vec<String>,
}

impl FiberSnapshot {
    /// Opaque correlation identity only; it grants no lookup or control power.
    pub fn id(&self) -> &FiberId {
        &self.id
    }
    /// Root versus ordinary Runtime role.
    pub fn role(&self) -> FiberRole {
        self.role
    }
    /// Display name of the Fiber.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Lifecycle state captured for this record.
    pub fn state(&self) -> FiberState {
        self.state
    }
    /// Missing Service names for this captured Pending record.
    pub fn missing_services(&self) -> &[String] {
        &self.missing_services
    }
}

/// One current occupied Service publication occurrence.
#[derive(Debug, Clone)]
pub struct ServiceSnapshot {
    pub(crate) id: ServicePublicationId,
    pub(crate) service: String,
    pub(crate) realm: ServiceRealm,
    pub(crate) provider: FiberId,
    pub(crate) visible: bool,
}

impl ServiceSnapshot {
    /// Opaque correlation identity only; it grants no mutation authority.
    pub fn id(&self) -> &ServicePublicationId {
        &self.id
    }
    /// Service contract name.
    pub fn service(&self) -> &str {
        &self.service
    }
    /// Exact Runtime-local Service realm.
    pub fn realm(&self) -> &ServiceRealm {
        &self.realm
    }
    /// Opaque provider Fiber correlation identity.
    pub fn provider(&self) -> &FiberId {
        &self.provider
    }
    /// Whether the occupied occurrence is currently lookup-visible.
    pub fn visible(&self) -> bool {
        self.visible
    }
}

pub(crate) fn fiber_snapshot(
    root: &crate::context::Root,
    fiber: &crate::fiber::Fiber,
) -> FiberSnapshot {
    let (state, missing_services) = loop {
        let state = fiber.state();
        let missing_services = if state == FiberState::Pending && fiber.is_alive() {
            fiber.missing_from(root)
        } else {
            Vec::new()
        };
        if fiber.state() == state {
            break (state, missing_services);
        }
    };
    FiberSnapshot {
        id: fiber.id().clone(),
        role: FiberRole::Ordinary,
        name: fiber.name.clone(),
        state,
        missing_services,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Context;
    use crate::fiber::Fiber;
    use crate::registry::PluginKey;

    #[test]
    fn disposed_record_remains_visible_until_exact_residency_unlink() {
        let ctx = Context::new();
        let fiber = Fiber::new("disposed-before-unlink");
        let id = fiber.id().clone();
        ctx.root
            .registry
            .attach_fiber(PluginKey::Anonymous(41), fiber.clone());

        // Issue 29 fixes the terminal order as Disposed before exact unlink.
        // Reproduce the Issue-41 state/residency relation without timing assumptions;
        // Issue 29 independently proves this state is published before unlink.
        fiber.set_state(FiberState::Disposed);

        let snapshot = ctx.runtime_snapshot();
        let record = snapshot
            .fibers()
            .iter()
            .find(|record| record.id() == &id)
            .unwrap();
        assert_eq!(record.role(), FiberRole::Ordinary);
        assert_eq!(record.state(), FiberState::Disposed);
        assert!(record.missing_services().is_empty());

        fiber.release_residency();
        assert!(
            ctx.runtime_snapshot()
                .fibers()
                .iter()
                .all(|record| record.id() != &id)
        );
    }

    #[derive(Debug)]
    struct ProbeService;
    impl crate::Service for ProbeService {
        const NAME: &'static str = "issue42/probe-service";
    }

    struct LoadingPublisher {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }
    impl crate::Plugin for LoadingPublisher {
        type Config = ();
        type Input = ();
        type PrepareError = std::convert::Infallible;
        type ApplyError = std::convert::Infallible;
        fn prepare(&self, (): ()) -> Result<(), Self::PrepareError> {
            Ok(())
        }
        async fn apply(&self, ctx: Context, _: &()) -> Result<(), Self::ApplyError> {
            let _publication = ctx.provide(Arc::new(ProbeService)).unwrap();
            self.entered.notify_one();
            self.release.notified().await;
            Ok(())
        }
    }

    #[tokio::test]
    async fn loading_install_is_not_visibility_but_active_and_unlink_are_committed_records() {
        let ctx = Context::new();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let task = tokio::spawn({
            let ctx = ctx.clone();
            let entered = entered.clone();
            let release = release.clone();
            async move {
                ctx.spawn(crate::PreparedPlugin::from_input(
                    LoadingPublisher { entered, release },
                    (),
                ))
                .await
                .unwrap()
            }
        });
        entered.notified().await;
        let loading = ctx.root.observations.take();
        assert!(loading.iter().any(|r| matches!(
            r,
            RuntimeObservation::FiberResidency {
                change: ResidencyChange::Admitted,
                ..
            }
        )));
        assert!(loading.iter().any(|r| matches!(
            r,
            RuntimeObservation::FiberState {
                previous: FiberState::Pending,
                current: FiberState::Loading,
                ..
            }
        )));
        assert!(
            !loading
                .iter()
                .any(|r| matches!(r, RuntimeObservation::ServiceVisibility { .. })),
            "Loading occupation alone is not Service visibility"
        );

        release.notify_one();
        let fiber_handle = task.await.unwrap();
        let active = ctx.root.observations.take();
        assert!(active.iter().any(|r| matches!(
            r,
            RuntimeObservation::ServiceVisibility {
                previous: None,
                current: Some(_),
                ..
            }
        )));
        assert!(active.iter().any(|r| matches!(
            r,
            RuntimeObservation::FiberState {
                previous: FiberState::Loading,
                current: FiberState::Active,
                ..
            }
        )));

        fiber_handle.dispose().await.unwrap();
        let disposed = ctx.root.observations.take();
        assert!(disposed.iter().any(|r| matches!(
            r,
            RuntimeObservation::ServiceVisibility {
                previous: Some(_),
                current: None,
                ..
            }
        )));
        assert_eq!(
            disposed
                .iter()
                .filter(|r| matches!(
                    r,
                    RuntimeObservation::FiberResidency {
                        change: ResidencyChange::Removed,
                        ..
                    }
                ))
                .count(),
            1
        );
    }

    struct Ping;
    impl crate::Event for Ping {
        const NAME: &'static str = "issue42/ping";
        type Args = u32;
        type Output = ();
    }
    struct Ask;
    impl crate::Event for Ask {
        const NAME: &'static str = "issue42/ask";
        type Args = u32;
        type Output = u32;
    }
    struct Flow;
    impl crate::Event for Flow {
        const NAME: &'static str = "issue42/flow";
        type Args = u32;
        type Output = u32;
    }

    #[tokio::test]
    async fn exact_listener_and_primitive_completion_use_semantic_vocabulary() {
        use crate::event::{ListenerOptions, observer_sync, responder_sync};
        let ctx = Context::new();
        let _once = ctx
            .on_with::<Ping, _>(
                observer_sync(|_, _| Ok::<_, std::convert::Infallible>(())),
                ListenerOptions::default().once(),
            )
            .unwrap();
        let _answer = ctx
            .on::<Ask, _>(responder_sync(|_, value| {
                Ok::<_, std::convert::Infallible>(Some(value + 1))
            }))
            .unwrap();
        ctx.root.observations.take();

        ctx.emit::<Ping>(Routing::Unscoped, 1).await.unwrap();
        let emitted = ctx.root.observations.take();
        assert!(emitted.iter().any(|r| matches!(r,
            RuntimeObservation::ListenerRegistration { change: ListenerChange::Unregistered, role: ListenerRole::Observer, options, .. }
                if options.is_once())));
        assert!(emitted.iter().any(|r| matches!(
            r,
            RuntimeObservation::DispatchCompleted {
                operation: EventOperation::Emit,
                outcome: DispatchOutcomeKind::Completed,
                ..
            }
        )));

        assert_eq!(
            ctx.query::<Ask>(Routing::Unscoped, 4).await.unwrap(),
            crate::QueryOutcome::Answer(5)
        );
        let answered = ctx.root.observations.take();
        assert!(answered.iter().any(|r| matches!(
            r,
            RuntimeObservation::DispatchCompleted {
                operation: EventOperation::Query,
                outcome: DispatchOutcomeKind::Answered,
                ..
            }
        )));

        let result = ctx
            .waterfall_query::<Flow, Ask, _, _, std::convert::Infallible>(
                Routing::Unscoped,
                7,
                |query| async move {
                    match query {
                        Ok(crate::QueryOutcome::Answer(value)) => Ok(value),
                        _ => Ok(0),
                    }
                },
            )
            .await
            .unwrap();
        assert_eq!(result, 8);
        let derived = ctx.root.observations.take();
        let operations = derived
            .iter()
            .filter_map(|record| match record {
                RuntimeObservation::DispatchCompleted { operation, .. } => Some(*operation),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            operations,
            vec![EventOperation::Query, EventOperation::Waterfall]
        );
    }

    struct Plain;
    impl crate::Plugin for Plain {
        type Config = ();
        type Input = ();
        type PrepareError = std::convert::Infallible;
        type ApplyError = std::convert::Infallible;
        fn prepare(&self, (): ()) -> Result<(), Self::PrepareError> {
            Ok(())
        }
        async fn apply(&self, _: Context, _: &()) -> Result<(), Self::ApplyError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn restart_preserves_residency_while_era_swap_replaces_it_and_manual_withdraw_is_exact() {
        let ctx = Context::new();
        let fiber_handle = ctx
            .spawn(crate::PreparedPlugin::from_input(Plain, ()))
            .await
            .unwrap();
        ctx.root.observations.take();

        fiber_handle.restart().await.unwrap();
        let restarted = ctx.root.observations.take();
        assert!(
            !restarted
                .iter()
                .any(|r| matches!(r, RuntimeObservation::FiberResidency { .. })),
            "same-Fiber restart is not a residency transition"
        );

        let old = fiber_handle.id();
        let successor = fiber_handle
            .era_swap(crate::PreparedChange::from_input::<Plain>(()))
            .await
            .unwrap();
        let replaced = ctx.root.observations.take();
        assert!(replaced.iter().any(|r| matches!(r,
            RuntimeObservation::FiberResidency { change: ResidencyChange::Removed, fiber } if fiber.id() == &old)));
        assert!(replaced.iter().any(|r| matches!(r,
            RuntimeObservation::FiberResidency { change: ResidencyChange::Admitted, fiber } if fiber.id() == &successor.id())));

        let publication = ctx.provide(Arc::new(ProbeService)).unwrap();
        let published = ctx.root.observations.take();
        assert!(published.iter().any(|r| matches!(
            r,
            RuntimeObservation::ServiceVisibility {
                previous: None,
                current: Some(_),
                ..
            }
        )));
        publication.remove().unwrap();
        let withdrawn = ctx.root.observations.take();
        assert!(withdrawn.iter().any(|r| matches!(
            r,
            RuntimeObservation::ServiceVisibility {
                previous: Some(_),
                current: None,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn all_primitive_operations_report_semantic_outcomes() {
        use crate::event::observer_sync;
        let ctx = Context::new();

        ctx.emit_parallel::<Ping>(Routing::Unscoped, 1)
            .await
            .unwrap();
        assert!(ctx.root.observations.take().iter().any(|r| matches!(
            r,
            RuntimeObservation::DispatchCompleted {
                operation: EventOperation::EmitParallel,
                outcome: DispatchOutcomeKind::Completed,
                ..
            }
        )));

        assert_eq!(
            ctx.query::<Ask>(Routing::Unscoped, 1).await.unwrap(),
            crate::QueryOutcome::Miss
        );
        assert!(ctx.root.observations.take().iter().any(|r| matches!(
            r,
            RuntimeObservation::DispatchCompleted {
                operation: EventOperation::Query,
                outcome: DispatchOutcomeKind::Missed,
                ..
            }
        )));

        let _failing = ctx
            .on::<Ping, _>(observer_sync(|_, _| {
                Err::<(), _>(std::io::Error::other("boom"))
            }))
            .unwrap();
        ctx.root.observations.take();
        assert!(
            ctx.emit_parallel::<Ping>(Routing::Unscoped, 2)
                .await
                .is_err()
        );
        assert!(ctx.root.observations.take().iter().any(|r| matches!(
            r,
            RuntimeObservation::DispatchCompleted {
                operation: EventOperation::EmitParallel,
                outcome: DispatchOutcomeKind::Failed,
                ..
            }
        )));

        let failed = ctx
            .waterfall::<Flow, _, _, std::io::Error>(Routing::Unscoped, 3, |_| async {
                Err(std::io::Error::other("tail failed"))
            })
            .await;
        assert!(failed.is_err());
        assert!(ctx.root.observations.take().iter().any(|r| matches!(
            r,
            RuntimeObservation::DispatchCompleted {
                operation: EventOperation::Waterfall,
                outcome: DispatchOutcomeKind::Failed,
                ..
            }
        )));
    }
}
