//! Services — typed, reactive dependency slots (mirrors `service.ts` +
//! `reflect.ts`).
//!
//! In cordis, services are values exposed on the context by name. The Rust
//! port makes the contract explicit: [`Service`] ties a type to a service
//! name; slots are stored per **Service realm** so Context derivation
//! ([`with_isolated_service`](crate::Context::with_isolated_service) /
//! [`with_service_realms`](crate::Context::with_service_realms))
//! privatizes them exactly like upstream isolation. Each exact
//! `(Service, ServiceRealm)` slot carries at most one current publication
//! occurrence. Loading occupation is invisible; Active publication is visible;
//! lifecycle close withdraws visibility before generation-owned cleanup removes
//! the physical row. Dependency targets use exact publication occurrence
//! identity rather than provider identity.

use crate::context::{Context, RealmKey, Root};
use crate::fiber::Fiber;
use parking_lot::Mutex;
use std::any::Any;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Weak};

#[cfg(test)]
struct VisiblePublishProbe {
    service: &'static str,
    reached: Arc<std::sync::Barrier>,
    resume: Arc<std::sync::Barrier>,
}

#[cfg(test)]
static VISIBLE_PUBLISH_PROBE: parking_lot::Mutex<Option<Arc<VisiblePublishProbe>>> =
    parking_lot::Mutex::new(None);

#[cfg(test)]
struct DriftKickProbe {
    root: Weak<Root>,
    armed: std::sync::atomic::AtomicBool,
    reached: Arc<std::sync::Barrier>,
    resume: Arc<std::sync::Barrier>,
}

#[cfg(test)]
static DRIFT_KICK_PROBE: parking_lot::Mutex<Option<Arc<DriftKickProbe>>> =
    parking_lot::Mutex::new(None);

#[cfg(test)]
fn probe_visible_publish(service: &'static str) {
    let probe = VISIBLE_PUBLISH_PROBE.lock().clone();
    if let Some(probe) = probe.filter(|probe| probe.service == service) {
        probe.reached.wait();
        probe.resume.wait();
    }
}

/// Private, non-zero-sized token proving a realm's Runtime membership.
pub(crate) struct RealmMembership {
    _identity: u8,
}

impl RealmMembership {
    pub(crate) fn new() -> Self {
        Self { _identity: 0 }
    }
}

/// An opaque Runtime-local placement identity for exact Service slots.
///
/// A realm carries no Service name, hierarchy, fallback rule, Event routing,
/// lifecycle ownership, or textual rendezvous policy. Allocate one through
/// [`Context::new_service_realm`] and install mappings atomically through
/// [`Context::with_service_realms`]. Realms from different Runtimes always
/// compare unequal.
#[derive(Clone)]
pub struct ServiceRealm {
    membership: Arc<RealmMembership>,
    key: RealmKey,
}

impl ServiceRealm {
    pub(crate) fn new(membership: Arc<RealmMembership>, key: RealmKey) -> Self {
        Self { membership, key }
    }

    pub(crate) fn belongs_to(&self, membership: &Arc<RealmMembership>) -> bool {
        Arc::ptr_eq(&self.membership, membership)
    }

    pub(crate) fn key(&self) -> RealmKey {
        self.key
    }
}

impl std::fmt::Debug for ServiceRealm {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ServiceRealm(..)")
    }
}

impl PartialEq for ServiceRealm {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && Arc::ptr_eq(&self.membership, &other.membership)
    }
}

impl Eq for ServiceRealm {}

impl Hash for ServiceRealm {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self.membership), state);
        self.key.hash(state);
    }
}

/// A complete Service-realm mapping batch was invalid.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum RealmMappingError {
    /// One Service name occurred more than once in the batch.
    #[error("service `{service}` is mapped more than once")]
    DuplicateService {
        /// The duplicated Service name.
        service: String,
    },
    /// A mapping used a realm allocated by another Runtime.
    #[error("service `{service}` uses a realm from another Runtime")]
    ForeignRealm {
        /// The Service name carrying the foreign realm.
        service: String,
    },
}

/// Marker trait linking a Rust type to one Runtime-local named Service contract.
pub trait Service: Send + Sync + 'static {
    /// The Service contract name.
    const NAME: &'static str;
}

/// A Service contract that owns typed source preparation and layer composition.
pub trait ConfigurableService: Service {
    /// Consumer-facing source configuration.
    type Config;
    /// One prepared configuration layer retained by the framework.
    type Layer: Send + Sync + 'static;
    /// The complete composed configuration delivered to the operation.
    type Resolved: Send + 'static;
    /// Failure while preparing source configuration.
    type PrepareError: std::error::Error;
    /// Failure while composing prepared layers.
    type ComposeError: std::error::Error;

    /// Validate and prepare one source configuration synchronously.
    fn prepare_config(config: Self::Config)
    -> std::result::Result<Self::Layer, Self::PrepareError>;

    /// Compose the optional base, ordered intercept layers, and optional head.
    fn compose_config<'a>(
        base: Option<&'a Self::Layer>,
        layers: impl IntoIterator<Item = &'a Self::Layer>,
        head: Option<&'a Self::Layer>,
    ) -> std::result::Result<Self::Resolved, Self::ComposeError>;
}

/// Failure to resolve one ConfigurableService's complete typed configuration.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigResolutionError<E: std::error::Error> {
    /// A same-name prepared layer belongs to another Service contract.
    #[error("configuration for service `{service}` belongs to a different contract")]
    ContractMismatch {
        /// The conflicting Service name.
        service: &'static str,
    },
    /// The Service contract rejected composition.
    #[error("service configuration composition failed: {0}")]
    Compose(E),
}

/// Exact visible lookup failure.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum ServiceLookupError {
    /// The exact selected realm has no visible publication.
    #[error("service `{service}` is unavailable")]
    Unavailable {
        /// The requested Service name.
        service: &'static str,
    },
    /// The Runtime already binds this name to another typed contract.
    #[error("service `{service}` belongs to a different contract")]
    ContractMismatch {
        /// The conflicting Service name.
        service: &'static str,
    },
}

/// Service publication admission failure.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum ServicePublishError {
    /// The current Context generation no longer admits registrations.
    #[error("the current Context generation is closed")]
    InactiveContext,
    /// The exact slot already has an eligible current publication.
    #[error("service `{service}` already has a publication in this realm")]
    DuplicatePublication {
        /// The occupied Service name.
        service: &'static str,
    },
    /// The Runtime already binds this name to another typed contract.
    #[error("service `{service}` belongs to a different contract")]
    ContractMismatch {
        /// The conflicting Service name.
        service: &'static str,
    },
}

/// Failure to control one exact Service publication occurrence.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum ServiceControlError {
    /// The capability no longer names the current occurrence in its exact slot.
    #[error("service `{service}` publication is stale")]
    StalePublication {
        /// The Service whose exact publication is no longer current.
        service: &'static str,
    },
    /// The exact occurrence is still current, but its generation no longer admits mutation.
    #[error("service `{service}` publication mutation is closed")]
    MutationClosed {
        /// The Service whose current publication can no longer be mutated.
        service: &'static str,
    },
}

/// Opaque identity of one successful Service publication occurrence.
///
/// Equality is allocation identity: every replacement receives a fresh value,
/// so stale cleanup can never compare equal to a later publication.
#[derive(Clone)]
pub(crate) struct ServiceOccurrenceId(Arc<u8>);

impl ServiceOccurrenceId {
    pub(crate) fn fresh() -> Self {
        Self(Arc::new(0))
    }
}

impl PartialEq for ServiceOccurrenceId {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for ServiceOccurrenceId {}
impl std::hash::Hash for ServiceOccurrenceId {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self.0), state);
    }
}

impl std::fmt::Debug for ServiceOccurrenceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ServiceOccurrenceId(..)")
    }
}

struct ServiceSlot {
    occurrence: ServiceOccurrenceId,
    value: Arc<dyn Any + Send + Sync>,
    contract: std::any::TypeId,
    fiber: Weak<Fiber>,
}

#[derive(Default)]
struct ServiceState {
    /// A Service name binds one typed contract for the Runtime lifetime.
    contracts: HashMap<&'static str, std::any::TypeId>,
    /// Exactly one current occupied occurrence per `(Service, realm)` slot.
    slots: HashMap<(RealmKey, &'static str), ServiceSlot>,
}

enum ServiceInstallRefusal {
    Duplicate,
    ContractMismatch,
}

/// Durable Service-drift obligations committed inside the ServiceStore semantic
/// mutation section. Driving them is deliberately deferred until after every
/// Service lock is released.
pub(crate) struct CommittedServiceDrift {
    affected: Vec<Arc<Fiber>>,
    /// When the derived dependency index is unavailable, retain the complete
    /// Registry snapshot until after the enclosing ServiceStore lock is gone.
    /// Filtering that snapshot inside the semantic commit must not run a
    /// potentially last-reference Fiber/plugin/config drop under ServiceStore
    /// synchronization (ADR 0029).
    retained_snapshot: Vec<Arc<Fiber>>,
}

impl CommittedServiceDrift {
    /// Called only from a ServiceStore semantic-mutation critical section.
    ///
    /// The proved nested bookkeeping direction is ServiceStore -> dependency
    /// projection/Registry. Those stores never call back into ServiceStore while
    /// holding their guards. The complete-index path already retains one Arc for
    /// every deduplicated Fiber before redundant clones drop; the fallback path
    /// explicitly retains every resident Arc until [`Self::kick`] runs outside
    /// ServiceStore synchronization.
    fn commit(root: &Root, edges: &[(String, RealmKey)]) -> Self {
        let (affected, retained_snapshot) = root.dependents_for_edges_with_retention(edges);
        for fiber in &affected {
            fiber.slot.commit_recheck();
        }
        Self {
            affected,
            retained_snapshot,
        }
    }

    pub(crate) fn kick(self, root: &Arc<Root>) {
        let Self {
            affected,
            retained_snapshot,
        } = self;
        #[cfg(test)]
        if let Some(probe) = DRIFT_KICK_PROBE.lock().clone()
            && probe.root.ptr_eq(&Arc::downgrade(root))
            && probe.armed.swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            probe.reached.wait();
            probe.resume.wait();
        }
        for fiber in affected {
            fiber.kick_committed_recheck(root);
            // Disposal can unlink this dependent after the semantic snapshot.
            // Its last Arc can then destroy a user Plugin or Input while the
            // provider still holds its lifecycle slot.
            let logger = root.logger.logger_for_fiber(&fiber.name);
            crate::contained::contain("Service dependent destruction", Some(&logger), || {
                drop(fiber)
            });
        }
        // The fallback is test-only today, but has the same ownership rule.
        // Keep each destructor separate so one panic cannot skip the rest.
        for fiber in retained_snapshot {
            let logger = root.logger.logger_for_fiber(&fiber.name);
            crate::contained::contain("Service snapshot destruction", Some(&logger), || {
                drop(fiber)
            });
        }
    }
}

struct ServiceSlotPublish<'a> {
    root: &'a Arc<Root>,
    owner: &'a Arc<Fiber>,
    store: &'a ServiceStore,
    key: RealmKey,
    name: &'static str,
    slot: Option<ServiceSlot>,
    evicted: Option<ServiceSlot>,
    drift: Option<CommittedServiceDrift>,
    refusal: Option<ServicePublishError>,
}

impl crate::gated::PublishStep for ServiceSlotPublish<'_> {
    fn publish(&mut self) -> std::result::Result<(), crate::gated::PublishRefused> {
        let slot = self.slot.take().expect("publish runs once");
        match self
            .store
            .install(self.root, self.owner, slot, self.key, self.name)
        {
            Ok((evicted, drift)) => {
                self.evicted = evicted;
                self.drift = drift;
                Ok(())
            }
            Err((refusal, slot)) => {
                self.slot = Some(slot);
                self.refusal = Some(match refusal {
                    ServiceInstallRefusal::Duplicate => {
                        ServicePublishError::DuplicatePublication { service: self.name }
                    }
                    ServiceInstallRefusal::ContractMismatch => {
                        ServicePublishError::ContractMismatch { service: self.name }
                    }
                });
                // The gated seam needs only an abort signal here. The exact
                // operation error is retained privately on this publish step
                // and returned by Context::provide after every lock is released.
                Err(crate::gated::PublishRefused)
            }
        }
    }
}

/// Runtime-local Service contract binding and exact-slot occurrence store.
#[derive(Default)]
pub(crate) struct ServiceStore {
    state: Mutex<ServiceState>,
}

impl ServiceStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn install(
        &self,
        root: &Root,
        owner: &Arc<Fiber>,
        slot: ServiceSlot,
        key: RealmKey,
        name: &'static str,
    ) -> std::result::Result<
        (Option<ServiceSlot>, Option<CommittedServiceDrift>),
        (ServiceInstallRefusal, ServiceSlot),
    > {
        let mut state = self.state.lock();
        if let Some(contract) = state.contracts.get(name) {
            if *contract != slot.contract {
                return Err((ServiceInstallRefusal::ContractMismatch, slot));
            }
        } else {
            state.contracts.insert(name, slot.contract);
        }

        if let Some(existing) = state.slots.get(&(key, name))
            && existing
                .fiber
                .upgrade()
                .is_some_and(|fiber| fiber.is_alive())
        {
            return Err((ServiceInstallRefusal::Duplicate, slot));
        }

        // Visibility and its durable affected-set obligation are one semantic
        // commit (ADR 0031). Commit the revision before mutating the slot while
        // holding the ServiceStore lock: target reconstruction cannot observe
        // the new slot until every pre-existing dependent already carries the
        // obligation. A dependent registered after this snapshot blocks on this
        // same lock in its initial target read and therefore sees the new slot.
        let drift = (owner.state() == crate::fiber::FiberState::Active)
            .then(|| CommittedServiceDrift::commit(root, &[(name.to_owned(), key)]));
        let evicted = state.slots.remove(&(key, name));
        state.slots.insert((key, name), slot);
        Ok((evicted, drift))
    }

    fn visible(slot: &ServiceSlot) -> bool {
        slot.fiber.upgrade().is_some_and(|fiber| {
            fiber.is_alive() && fiber.state() == crate::fiber::FiberState::Active
        })
    }

    pub(crate) fn occurrence_id(&self, key: &RealmKey, name: &str) -> Option<ServiceOccurrenceId> {
        let state = self.state.lock();
        let slot = state.slots.get(&(*key, name))?;
        Self::visible(slot).then(|| slot.occurrence.clone())
    }

    fn visible_value<S: Service>(
        &self,
        key: RealmKey,
    ) -> std::result::Result<Arc<S>, ServiceLookupError> {
        let value = {
            let state = self.state.lock();
            match state.contracts.get(S::NAME) {
                Some(contract) if *contract != std::any::TypeId::of::<S>() => {
                    return Err(ServiceLookupError::ContractMismatch { service: S::NAME });
                }
                _ => {}
            }
            let Some(slot) = state
                .slots
                .get(&(key, S::NAME))
                .filter(|slot| Self::visible(slot))
            else {
                return Err(ServiceLookupError::Unavailable { service: S::NAME });
            };
            slot.value.clone()
        };
        value
            .downcast::<S>()
            .map_err(|_| ServiceLookupError::ContractMismatch { service: S::NAME })
    }

    fn set_exact(
        &self,
        key: RealmKey,
        name: &'static str,
        occurrence: &ServiceOccurrenceId,
        owner: &Weak<Fiber>,
        value: Arc<dyn Any + Send + Sync>,
    ) -> std::result::Result<
        Arc<dyn Any + Send + Sync>,
        (ServiceControlError, Arc<dyn Any + Send + Sync>),
    > {
        let mut state = self.state.lock();
        let Some(slot) = state.slots.get_mut(&(key, name)) else {
            return Err((
                ServiceControlError::StalePublication { service: name },
                value,
            ));
        };
        if slot.occurrence != *occurrence {
            return Err((
                ServiceControlError::StalePublication { service: name },
                value,
            ));
        }
        let fiber = owner.upgrade();
        let _gate = fiber
            .as_ref()
            .map(|fiber| fiber.service_mutation_gate.lock());
        if !fiber
            .as_ref()
            .is_some_and(|fiber| fiber.assert_can_register().is_ok())
        {
            return Err((ServiceControlError::MutationClosed { service: name }, value));
        }
        Ok(std::mem::replace(&mut slot.value, value))
    }

    fn remove_exact(
        &self,
        root: &Root,
        key: RealmKey,
        name: &'static str,
        occurrence: &ServiceOccurrenceId,
        owner: &Weak<Fiber>,
    ) -> std::result::Result<(ServiceSlot, Option<CommittedServiceDrift>), ServiceControlError>
    {
        let mut state = self.state.lock();
        let entry = match state.slots.entry((key, name)) {
            std::collections::hash_map::Entry::Occupied(entry)
                if entry.get().occurrence == *occurrence =>
            {
                entry
            }
            _ => return Err(ServiceControlError::StalePublication { service: name }),
        };
        let fiber = owner.upgrade();
        let _gate = fiber
            .as_ref()
            .map(|fiber| fiber.service_mutation_gate.lock());
        let Some(owner_state) = fiber
            .as_ref()
            .filter(|fiber| fiber.assert_can_register().is_ok())
            .map(|fiber| fiber.state())
        else {
            return Err(ServiceControlError::MutationClosed { service: name });
        };
        let drift = (owner_state == crate::fiber::FiberState::Active).then(|| {
            // Commit before removing the visible slot while the ServiceStore
            // lock excludes target reconstruction. This closes the same
            // publication-to-revision window as active late publication.
            CommittedServiceDrift::commit(root, &[(name.to_owned(), key)])
        });
        Ok((entry.remove(), drift))
    }

    fn withdraw_exact(&self, key: RealmKey, name: &'static str, occurrence: &ServiceOccurrenceId) {
        let removed = {
            let mut state = self.state.lock();
            match state.slots.entry((key, name)) {
                std::collections::hash_map::Entry::Occupied(entry)
                    if entry.get().occurrence == *occurrence =>
                {
                    Some(entry.remove())
                }
                _ => None,
            }
        };
        // The slot contains an Arc<S>; destroy it only after bookkeeping
        // synchronization is released.
        drop(removed);
    }

    /// Exact occupied slots owned by `fiber`. Called immediately after a
    /// lifecycle state transition; because visibility is exactly Active,
    /// entering or leaving Active makes precisely these slots change visibility.
    pub(crate) fn slots_owned_by(&self, fiber: &Arc<Fiber>) -> Vec<(String, RealmKey)> {
        let owner = Arc::downgrade(fiber);
        self.state
            .lock()
            .slots
            .iter()
            .filter(|(_, slot)| Weak::ptr_eq(&slot.fiber, &owner))
            .map(|((key, name), _)| ((*name).to_owned(), *key))
            .collect()
    }

    /// Publish an Active-visibility lifecycle boundary together with the
    /// complete dependent recheck obligation in one ServiceStore critical
    /// section (ADR 0031). Target reconstruction takes this same lock, so it
    /// cannot observe the new provider state before pre-existing dependents
    /// carry the matching durable revision.
    pub(crate) fn commit_fiber_transition(
        &self,
        root: &Root,
        fiber: &Arc<Fiber>,
        old: crate::fiber::FiberState,
        next: crate::fiber::FiberState,
    ) -> (
        Vec<ServiceVisibilityOccurrence>,
        Option<CommittedServiceDrift>,
    ) {
        debug_assert_ne!(old, next);
        debug_assert_ne!(
            old == crate::fiber::FiberState::Active,
            next == crate::fiber::FiberState::Active,
            "only Active visibility boundaries use the Service semantic commit"
        );

        let state = self.state.lock();
        let owner = Arc::downgrade(fiber);
        let visibility = state
            .slots
            .iter()
            .filter(|(_, slot)| Weak::ptr_eq(&slot.fiber, &owner))
            .map(|((key, name), slot)| ServiceVisibilityOccurrence {
                service: (*name).to_owned(),
                realm: *key,
                occurrence: slot.occurrence.clone(),
            })
            .collect::<Vec<_>>();
        let drift = (!visibility.is_empty()).then(|| {
            let edges = visibility
                .iter()
                .map(|slot| (slot.service.clone(), slot.realm))
                .collect::<Vec<_>>();
            CommittedServiceDrift::commit(root, &edges)
        });

        // Revision first, visibility second, under the one lock target readers
        // also take. A holder that already read the old target will observe the
        // new revision and retry; a holder retrying after that observation blocks
        // here until the new provider state is visible.
        fiber.set_state(next);
        drop(state);
        (visibility, drift)
    }

    pub(crate) fn snapshot_occurrences(&self) -> Vec<ServiceOccurrenceSnapshot> {
        let state = self.state.lock();
        state
            .slots
            .iter()
            .filter_map(|((realm, service), slot)| {
                let fiber = slot.fiber.upgrade()?;
                let fiber_state = fiber.state();
                // Current publication truth follows the same generation gate as
                // mutation/registration. A restart/dispose closes that gate
                // synchronously before the later Unloading publication, so an
                // Active-looking physical row can already be stale.
                let current = fiber.assert_can_register().is_ok();
                current.then(|| ServiceOccurrenceSnapshot {
                    id: slot.occurrence.clone(),
                    service: (*service).to_owned(),
                    realm: *realm,
                    provider: fiber.id().clone(),
                    visible: fiber_state == crate::fiber::FiberState::Active,
                })
            })
            .collect()
    }
}

pub(crate) struct ServiceVisibilityOccurrence {
    pub(crate) service: String,
    pub(crate) realm: RealmKey,
    pub(crate) occurrence: ServiceOccurrenceId,
}

pub(crate) struct ServiceOccurrenceSnapshot {
    pub(crate) id: ServiceOccurrenceId,
    pub(crate) service: String,
    pub(crate) realm: RealmKey,
    pub(crate) provider: crate::fiber::FiberId,
    pub(crate) visible: bool,
}

/// Move-only opaque capability naming one exact Service publication occurrence.
///
/// The capability is the sole mutation authority for the occurrence that created
/// it. Dropping it is inert: the owning generation retains its cleanup claim.
#[must_use = "dropping a ServicePublication leaves generation-owned publication cleanup armed"]
pub struct ServicePublication<S: Service> {
    root: Arc<Root>,
    key: RealmKey,
    occurrence: ServiceOccurrenceId,
    owner: Weak<Fiber>,
    cleanup: crate::effect::DisposableToken,
    _service: std::marker::PhantomData<fn() -> S>,
}

impl<S: Service> ServicePublication<S> {
    /// Replace only this exact publication occurrence's payload.
    ///
    /// Publication identity is preserved, so a successful set does not create
    /// dependency-target drift. Exact staleness is checked before generation
    /// closure. Rejected incoming values and replaced outgoing values are
    /// destroyed only after Service synchronization is released.
    pub fn set(&self, value: Arc<S>) -> std::result::Result<(), ServiceControlError> {
        let erased: Arc<dyn Any + Send + Sync> = value;
        match self
            .root
            .services
            .set_exact(self.key, S::NAME, &self.occurrence, &self.owner, erased)
        {
            Ok(outgoing) => {
                drop(outgoing);
                Ok(())
            }
            Err((error, rejected)) => {
                drop(rejected);
                Err(error)
            }
        }
    }

    /// Consume this capability and remove only its exact publication occurrence.
    ///
    /// A successful visible withdrawal creates dependency drift once. The
    /// generation-owned cleanup claim is disarmed when still available; if the
    /// generation drain already claimed it, its exact cleanup becomes a stale
    /// no-op. Stale identity is reported before a closed-generation error.
    pub fn remove(self) -> std::result::Result<(), ServiceControlError> {
        let (removed, drift) = self.root.services.remove_exact(
            &self.root,
            self.key,
            S::NAME,
            &self.occurrence,
            &self.owner,
        )?;
        let was_visible = drift.is_some();

        let claimed_cleanup = self
            .owner
            .upgrade()
            .and_then(|fiber| fiber.remove_disposable(self.cleanup));

        if let Some(drift) = drift {
            // Executor-dependent driving stays outside the ServiceStore lock;
            // the durable obligation was already committed with the removal.
            drift.kick(&self.root);
        }
        if was_visible {
            self.root.observations.publish(
                crate::observation::RuntimeObservation::ServiceVisibility {
                    service: S::NAME.to_owned(),
                    realm: ServiceRealm::new(self.root.realm_membership.clone(), self.key),
                    previous: Some(crate::observation::ServicePublicationId(
                        self.occurrence.clone(),
                    )),
                    current: None,
                },
            );
        }

        // Both values can own user-authored Service payloads or captures. Every
        // framework lock is already released before either destruction occurs.
        drop(claimed_cleanup);
        drop(removed);
        Ok(())
    }
}

impl Context {
    /// Publish one generation-owned occurrence into the exact Service slot
    /// selected by this Context's isolate axis.
    pub fn provide<S: Service>(
        &self,
        value: Arc<S>,
    ) -> std::result::Result<ServicePublication<S>, ServicePublishError> {
        let fiber = self.fiber().clone();
        fiber
            .assert_can_register()
            .map_err(|_| ServicePublishError::InactiveContext)?;
        let key = self.isolate_key(S::NAME);
        let occurrence = ServiceOccurrenceId::fresh();
        let cleanup_root = self.root.clone();
        let cleanup_occurrence = occurrence.clone();
        let mut step = ServiceSlotPublish {
            root: &self.root,
            owner: &fiber,
            store: &self.root.services,
            key,
            name: S::NAME,
            slot: Some(ServiceSlot {
                occurrence: occurrence.clone(),
                value,
                contract: std::any::TypeId::of::<S>(),
                fiber: Arc::downgrade(&fiber),
            }),
            evicted: None,
            drift: None,
            refusal: None,
        };
        let committed = crate::gated::push_gated(
            &fiber,
            crate::effect::sync_cleanup(move || {
                // Gate close already made an Active occurrence invisible and
                // emitted its drift. Physical stale cleanup is identity-checked
                // and semantically inert.
                cleanup_root
                    .services
                    .withdraw_exact(key, S::NAME, &cleanup_occurrence);
            }),
            &mut step,
        );
        let cleanup = match committed {
            Ok(cleanup) => cleanup,
            Err(_) => {
                return Err(step
                    .refusal
                    .take()
                    .unwrap_or(ServicePublishError::InactiveContext));
            }
        };
        #[cfg(test)]
        if step.drift.is_some() {
            // Deterministic seam for the ADR 0031 regression: the ServiceStore
            // mutation and durable revisions have committed, but executor-
            // dependent kicks have not run yet.
            probe_visible_publish(S::NAME);
        }
        let drift = step.drift.take();
        let became_visible = drift.is_some();
        drop(step);

        // Loading installation is occupied but invisible. Active late/root
        // publication committed its dependent revisions inside ServiceStore;
        // only executor-dependent driving and observation remain here.
        if let Some(drift) = drift {
            drift.kick(&self.root);
        }
        if became_visible {
            self.root.observations.publish(
                crate::observation::RuntimeObservation::ServiceVisibility {
                    service: S::NAME.to_owned(),
                    realm: ServiceRealm::new(self.root.realm_membership.clone(), key),
                    previous: None,
                    current: Some(crate::observation::ServicePublicationId(occurrence.clone())),
                },
            );
        }

        Ok(ServicePublication {
            root: self.root.clone(),
            key,
            occurrence,
            owner: Arc::downgrade(&fiber),
            cleanup,
            _service: std::marker::PhantomData,
        })
    }

    /// Resolve only the exact Service slot selected by this Context's isolate
    /// axis. InjectSpec membership is irrelevant and no fallback/provider
    /// predicate is consulted.
    pub fn try_service<S: Service>(&self) -> std::result::Result<Arc<S>, ServiceLookupError> {
        let key = self.isolate_key(S::NAME);
        self.root.services.visible_value::<S>(key)
    }
}

#[cfg(test)]
mod semantic_commit_tests {
    use super::{DRIFT_KICK_PROBE, DriftKickProbe, VISIBLE_PUBLISH_PROBE, VisiblePublishProbe};
    use crate::logger::{BufferExporter, Level};
    use crate::{Context, FiberState, InjectSpec, Plugin, PreparedPlugin, Service};
    use std::convert::Infallible;
    use std::future::Future;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Panic only when the retained snapshot releases the dependent's final Arc.
    struct PanicOnFinalInputDrop(Arc<std::sync::atomic::AtomicBool>);
    impl Drop for PanicOnFinalInputDrop {
        fn drop(&mut self) {
            if self.0.load(Ordering::SeqCst) {
                panic!("dependent input last Arc dropped during provider restart");
            }
        }
    }

    struct DropDependent;
    impl Plugin for DropDependent {
        type Config = ();
        type Input = PanicOnFinalInputDrop;
        type PrepareError = Infallible;
        type ApplyError = Infallible;
        fn inject(&self) -> InjectSpec {
            InjectSpec::none().require(AtomicVisibility::NAME)
        }
        fn prepare(&self, _: ()) -> Result<Self::Input, Infallible> {
            unreachable!()
        }
        async fn apply(&self, _: Context, _: &Self::Input) -> Result<(), Infallible> {
            Ok(())
        }
    }

    struct PublicProvider;
    impl Plugin for PublicProvider {
        type Config = ();
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = Infallible;
        fn prepare(&self, _: ()) -> Result<(), Infallible> {
            Ok(())
        }
        async fn apply(&self, ctx: Context, _: &()) -> Result<(), Infallible> {
            let _publication = ctx.provide(Arc::new(AtomicVisibility)).unwrap();
            Ok(())
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn disposed_dependent_last_arc_does_not_orphan_provider_restart() {
        let root = Context::new();
        let buffer = Arc::new(BufferExporter::new(8, Level::Debug).unwrap());
        let _exporter = root.add_exporter(buffer.clone()).unwrap();
        let provider = root
            .spawn(PreparedPlugin::from_input(PublicProvider, ()))
            .await
            .unwrap();
        let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let dependent = root
            .spawn(PreparedPlugin::from_input(
                DropDependent,
                PanicOnFinalInputDrop(armed.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(dependent.state(), FiberState::Active);
        let survivor = root
            .spawn(PreparedPlugin::from_input(
                WantsAtomicVisibility(Arc::new(AtomicUsize::new(0))),
                (),
            ))
            .await
            .unwrap();
        assert_eq!(survivor.state(), FiberState::Active);
        let weak = Arc::downgrade(&dependent.fiber);
        let reached = Arc::new(std::sync::Barrier::new(2));
        let resume = Arc::new(std::sync::Barrier::new(2));
        // Pause the public restart after its Service transition has retained
        // the dependent, before the snapshot Arc is released.
        *DRIFT_KICK_PROBE.lock() = Some(Arc::new(DriftKickProbe {
            root: Arc::downgrade(&root.root),
            armed: std::sync::atomic::AtomicBool::new(true),
            reached: reached.clone(),
            resume: resume.clone(),
        }));
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                *DRIFT_KICK_PROBE.lock() = None;
            }
        }
        let _reset = Reset;
        let restarting = provider.clone();
        let task = tokio::spawn(async move { restarting.restart().await });
        tokio::task::spawn_blocking(move || reached.wait())
            .await
            .unwrap();
        dependent.dispose().await.unwrap();
        drop(dependent);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while weak.strong_count() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("semantic snapshot is the dependent's last Arc");
        armed.store(true, Ordering::SeqCst);
        tokio::task::spawn_blocking(move || resume.wait())
            .await
            .unwrap();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .expect("provider restart owner must finish")
            .expect("dependent Input Drop must be contained");
        outcome.unwrap();
        assert_eq!(provider.ready().await.unwrap(), FiberState::Active);
        assert_eq!(survivor.ready().await.unwrap(), FiberState::Active);
        let reports = buffer
            .snapshot()
            .into_iter()
            .filter(|record| {
                record
                    .text()
                    .contains("Service dependent destruction panicked")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            reports.len(),
            1,
            "one routed diagnostic per destructor panic"
        );
        assert_eq!(reports[0].level(), Level::Warn);
        assert!(
            reports[0]
                .text()
                .contains("dependent input last Arc dropped during provider restart")
        );
        assert!(weak.upgrade().is_none());
        survivor.dispose().await.unwrap();
        provider.dispose().await.unwrap();
    }

    struct AtomicVisibility;

    impl Service for AtomicVisibility {
        const NAME: &'static str = "issue101/atomic-visibility";
    }

    struct WantsAtomicVisibility(Arc<AtomicUsize>);

    impl Plugin for WantsAtomicVisibility {
        type Config = ();
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = Infallible;

        fn inject(&self) -> InjectSpec {
            InjectSpec::none().require(AtomicVisibility::NAME)
        }

        fn prepare(&self, _config: ()) -> Result<(), Infallible> {
            Ok(())
        }

        fn apply(
            &self,
            ctx: Context,
            _prepared: &(),
        ) -> impl Future<Output = Result<(), Infallible>> + Send {
            let applies = self.0.clone();
            async move {
                ctx.try_service::<AtomicVisibility>()
                    .expect("committed visible Service is available to the dependent");
                applies.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }
    }

    struct ProbeReset;

    impl Drop for ProbeReset {
        fn drop(&mut self) {
            *VISIBLE_PUBLISH_PROBE.lock() = None;
        }
    }

    #[tokio::test]
    async fn visible_publish_commits_recheck_before_kick_window() {
        let root = Context::new();
        let applies = Arc::new(AtomicUsize::new(0));
        let dependent = root
            .spawn(PreparedPlugin::from_input(
                WantsAtomicVisibility(applies.clone()),
                (),
            ))
            .await
            .unwrap();
        assert_eq!(dependent.state(), FiberState::Pending);

        let reached = Arc::new(std::sync::Barrier::new(2));
        let resume = Arc::new(std::sync::Barrier::new(2));
        *VISIBLE_PUBLISH_PROBE.lock() = Some(Arc::new(VisiblePublishProbe {
            service: AtomicVisibility::NAME,
            reached: reached.clone(),
            resume: resume.clone(),
        }));
        let _reset = ProbeReset;

        let provider = std::thread::spawn({
            let root = root.clone();
            move || {
                let _publication = root.provide(Arc::new(AtomicVisibility)).unwrap();
            }
        });
        reached.wait();

        // The Service occurrence is already published, but its ordinary kick is
        // deliberately paused. ADR 0031 requires the durable recheck to have
        // committed in the same semantic transaction, so ready() must discover
        // the obligation, drive it itself, and converge to the visible target.
        // The historical ordering (publish -> later commit_recheck) returned
        // Pending here even though ServiceStore already exposed the occurrence.
        let ready = crate::deadline::bounded(2000, dependent.ready()).await;

        resume.wait();
        provider.join().unwrap();

        assert_eq!(
            ready
                .expect("ready drives the already-committed recheck")
                .unwrap(),
            FiberState::Active
        );
        assert_eq!(applies.load(Ordering::SeqCst), 1);
        root.try_service::<AtomicVisibility>()
            .expect("dropping the publication capability is inert");
    }
}
