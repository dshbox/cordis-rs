//! [`Context`] — a cheap immutable view into one Runtime.
//!
//! Behavior is determined only by the shared Runtime, current Fiber, and the
//! three semantic axes carried by the view: isolate, Scope, and intercept.
//! There is no generic Context ancestry or hierarchy for another subsystem to
//! inspect. Each primitive derivation changes only its named axis.
//!
//! - `root: Arc<Root>` — shared Runtime state;
//! - `fiber` — the current Fiber that owns registrations through this view;
//! - `isolate` — the Service-to-realm position;
//! - `scope` — the rooted Event-reachability position;
//! - `intercept` — the ordered Service-configuration position.
//!
//! Cloning copies those positions exactly. Private persistence structures inside
//! an individual axis are representation only and grant no cross-axis meaning.
//!
//! [`fiber`]: crate::fiber
//!
//! Listeners receive a [`Context`] as their first parameter — the
//! registration-time context, cloned per invocation (ADR 0009) — so a
//! listener never needs to capture one.

use crate::events::EventStore;
use crate::fiber::Fiber;
use crate::registry::Registry;
use crate::service::ServiceStore;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Private storage key behind an opaque [`crate::ServiceRealm`].
///
/// Keys are allocated from the owning Runtime's counter. `0` is its default
/// realm and allocated realm keys start at 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RealmKey(u64);

impl RealmKey {
    /// The Runtime-local default realm storage key.
    pub(crate) const DEFAULT: Self = Self(0);
}

/// Runtime-local counter for private Service-realm storage keys.
///
/// Each [`Context::new`] owns a fresh counter. Allocated realm keys start at 1;
/// `0` remains the default realm sentinel.
#[derive(Debug, Default)]
pub(crate) struct SnCounters {
    /// Private storage keys for allocated Service realms.
    realm: AtomicU64,
}

impl SnCounters {
    /// Allocated realm keys start at 1 because the default realm uses 0.
    pub(crate) fn next_realm_key(&self) -> RealmKey {
        let previous = self
            .realm
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |key| {
                key.checked_add(1)
            })
            .expect("Service realm identity space exhausted");
        RealmKey(previous + 1)
    }
}

/// Per-runtime state shared by every context handle of one runtime.
pub(crate) struct Root {
    /// The runtime-wide hook storage (mirrors upstream `EventsService`).
    pub(crate) events: Arc<EventStore>,
    pub(crate) observations: Arc<crate::observation::ObservationHub>,
    pub(crate) updates: Arc<crate::update::UpdateStore>,
    /// Plugin runtimes: where spawned fibers are registered (residency).
    pub(crate) registry: Registry,
    /// Optional reverse acceleration for Fiber-owned exact dependency edges.
    /// Correctness falls back to Registry-resident Fiber authority.
    pub(crate) deps: crate::deps::DependencyIndex,
    /// Service slots SemanticTargets are reconstructed from.
    pub(crate) services: ServiceStore,
    /// The runtime-wide logging state: sequence counters and the
    /// exporter list every [`Context::logger`](crate::Context::logger)
    /// channel feeds (mirrors upstream `LoggerService`).
    pub(crate) logger: Arc<crate::logger::LoggerService>,
    /// Unforgeable membership identity shared by every realm from this Runtime.
    pub(crate) realm_membership: Arc<crate::service::RealmMembership>,
    /// Opaque Runtime membership carried by public Event Scope capabilities.
    pub(crate) scope_membership: Arc<ScopeMembership>,
    /// The runtime's id space.
    pub(crate) sn: SnCounters,
    /// The ever-active root fiber: backs root-context registrations so they
    /// live for the runtime's lifetime (mirrors upstream's permanently
    /// ACTIVE root fiber, `fiber.ts:200-212`).
    pub(crate) root_fiber: Arc<Fiber>,
}

impl Root {
    pub(crate) fn dependents_for_edges(&self, edges: &[(String, RealmKey)]) -> Vec<Arc<Fiber>> {
        if let Some(indexed) = self.deps.dependents_of(edges) {
            return indexed;
        }
        let requested = edges.iter().cloned().collect::<HashSet<_>>();
        self.registry
            .snapshot_fibers()
            .into_iter()
            .filter(|fiber| {
                fiber.is_alive()
                    && fiber
                        .dependency_edges()
                        .iter()
                        .any(|edge| requested.contains(&(edge.service.clone(), edge.realm)))
            })
            .collect()
    }
}

/// A handle onto one cordis runtime: listener registration, dispatch,
/// effects, services, and plugin spawn.
///
/// Cheap to clone. All clones of one Runtime view share Runtime state while
/// preserving the same current Fiber and semantic axis positions.
#[derive(Clone)]
pub struct Context {
    pub(crate) root: Arc<Root>,
    /// The fiber this handle registers through. Always set: `new()`/`root`
    /// carry the root fiber, plugin apply contexts carry the plugin's
    /// fiber.
    pub(crate) fiber: Arc<Fiber>,
    /// Isolate shadowing layers; `None` selects the default Service realm.
    pub(crate) isolate: Option<Arc<IsolateLayer>>,
    /// This handle's position in the Scope chain; `None` is the root Scope.
    /// `Routing::Scoped` filters delivery by reachability on this chain.
    pub(crate) scope: Option<Arc<ScopeNode>>,
    /// Ordered intercept position; `None` = no intercepts declared. Matching
    /// Service layers are folded outer-to-inner by `resolve_config`.
    pub(crate) intercept: Option<Arc<InterceptLayer>>,
}

impl Context {
    /// Select `fiber` as the current Fiber while preserving all three semantic
    /// axes exactly. Effects, listeners, and services registered through the
    /// result attach to that Fiber's current open generation.
    pub(crate) fn with_fiber(&self, fiber: Arc<Fiber>) -> Context {
        Context {
            root: self.root.clone(),
            fiber,
            isolate: self.isolate.clone(),
            scope: self.scope.clone(),
            intercept: self.intercept.clone(),
        }
    }

    /// Create a fresh runtime and its root context.
    ///
    /// Every [`Context`] derived from this one shares the same hook store,
    /// services, and registry — a listener registered through any handle of
    /// the runtime dispatches through all of them. Registrations made on
    /// the returned handle itself attach to the permanently active root Fiber.
    /// The view selects the Runtime's default Service realm, root Scope, and
    /// empty intercept position.
    #[allow(
        clippy::new_without_default,
        reason = "v3 requires explicit Runtime construction and removes Context::default"
    )]
    pub fn new() -> Self {
        let sn = SnCounters::default();
        let root_fiber = Fiber::root();
        let root = Arc::new(Root {
            events: Arc::new(EventStore::new()),
            observations: Arc::new(crate::observation::ObservationHub::default()),
            updates: Arc::new(crate::update::UpdateStore::new()),
            registry: Registry::new(),
            deps: crate::deps::DependencyIndex::new(),
            services: ServiceStore::new(),
            logger: Arc::new(crate::logger::LoggerService::new()),
            realm_membership: Arc::new(crate::service::RealmMembership::new()),
            scope_membership: Arc::new(ScopeMembership),
            sn,
            root_fiber,
        });
        Self {
            fiber: root.root_fiber.clone(),
            isolate: None,
            scope: None,
            intercept: None,
            root,
        }
    }

    /// The Runtime's root Context: a live handle at the root Scope, carrying
    /// the default isolate/intercept positions and permanently active root Fiber
    /// — registrations through it live for the runtime's lifetime
    /// (mirrors upstream `ctx.root`, `context.ts:40`).
    ///
    /// Dropping the returned value is almost always a bug — derive and use
    /// it. Reachable from any derived view, this is how an isolated Context
    /// returns to the default Service realm.
    #[must_use = "a derived context that is dropped has no effect"]
    pub fn root(&self) -> Self {
        Self {
            root: self.root.clone(),
            fiber: self.root.root_fiber.clone(),
            isolate: None,
            scope: None,
            intercept: None,
        }
    }

    /// Subscribe to immutable postcommit Runtime observations.
    /// Delivery is detached, Runtime-wide, unscoped, parallel, attempt-all, and best-effort.
    pub fn observe_runtime<O>(
        &self,
        observer: O,
    ) -> std::result::Result<(), crate::effect::EffectRegistrationError>
    where
        O: crate::observation::RuntimeObserver,
    {
        self.root.observations.register(self, observer)
    }

    /// Capture a flat read-only projection of this Runtime's current state.
    ///
    /// Records are individually self-consistent, but the Fiber and Service
    /// collections are unordered, are not one globally linearizable instant,
    /// and need not be referentially closed. The snapshot is current-state
    /// diagnostics only: it provides no history, replay, lookup, or control.
    pub fn runtime_snapshot(&self) -> crate::observation::RuntimeSnapshot {
        use crate::fiber::{FiberRole, FiberState};
        use crate::observation::{FiberSnapshot, RuntimeSnapshot, ServiceSnapshot};

        let mut fibers = Vec::new();
        fibers.push(FiberSnapshot {
            id: self.root.root_fiber.id().clone(),
            role: FiberRole::Root,
            name: self.root.root_fiber.name.clone(),
            state: FiberState::Active,
            missing_services: Vec::new(),
        });
        fibers.extend(
            self.root
                .registry
                .snapshot_fibers()
                .into_iter()
                .map(|fiber| crate::observation::fiber_snapshot(&self.root, &fiber)),
        );

        let services = self
            .root
            .services
            .snapshot_occurrences()
            .into_iter()
            .map(|record| ServiceSnapshot {
                id: crate::observation::ServicePublicationId(record.id),
                service: record.service,
                realm: crate::ServiceRealm::new(self.root.realm_membership.clone(), record.realm),
                provider: record.provider,
                visible: record.visible,
            })
            .collect();

        RuntimeSnapshot { fibers, services }
    }

    /// The fiber this handle registers through (crate-internal callers
    /// only; consumers observe fibers through [`FiberHandle`](crate::FiberHandle)).
    pub(crate) fn fiber(&self) -> &Arc<Fiber> {
        &self.fiber
    }

    /// Resolve the exact storage key selected by this view's isolate position.
    /// The nearest explicit mapping wins; an unmapped name selects the Runtime's
    /// default realm. No Context, Scope, or lifecycle ancestry participates.
    pub(crate) fn isolate_key(&self, name: &str) -> RealmKey {
        self.isolate
            .as_deref()
            .and_then(|layer| layer.lookup(name))
            .unwrap_or(RealmKey::DEFAULT)
    }

    /// Allocate one fresh opaque Service realm in this Context's Runtime.
    ///
    /// A realm is only a placement identity. It carries no Service name,
    /// hierarchy, fallback rule, Event reachability, or textual rendezvous
    /// policy. Use [`Context::with_service_realms`] to map Service names to
    /// allocated realms.
    pub fn new_service_realm(&self) -> crate::ServiceRealm {
        crate::ServiceRealm::new(
            self.root.realm_membership.clone(),
            self.root.sn.next_realm_key(),
        )
    }

    /// Derive a Context where the Service literally named `name` resolves to
    /// one fresh private realm — the day-one successor of upstream
    /// `ctx.isolate(name)` (`context.ts:65-69`; the rename is ADR 0008).
    ///
    /// **`name` is a Service name, not a realm label.** It selects which
    /// Service the derived view maps; every other Service keeps its inherited
    /// mapping or the Runtime's default realm. Calling `with_isolated_service`
    /// twice yields two independent realms for that Service.
    ///
    /// The tempting `with_isolated_service("tenant-a")` reading isolates
    /// nothing: no service is named `"tenant-a"`, so two providers of
    /// e.g. `cache` spawned from such views land in one default-realm slot and
    /// the second fails with
    /// [`ServicePublishError::DuplicatePublication`](crate::service::ServicePublishError::DuplicatePublication).
    /// Isolate the service, not the tenant:
    ///
    /// ```
    /// use cordis_core::{Context, Service};
    /// use std::sync::Arc;
    ///
    /// struct Cache { tenant: &'static str }
    /// impl Service for Cache { const NAME: &'static str = "cache"; }
    ///
    /// # fn main() -> Result<(), cordis_core::BoxError> {
    /// let ctx = Context::new();
    ///
    /// // The tenant is the handle; the argument names the service whose
    /// // slot to privatize. Same name, two private slots.
    /// let tenant_a = ctx.with_isolated_service(Cache::NAME);
    /// let tenant_b = ctx.with_isolated_service(Cache::NAME);
    ///
    /// tenant_a.provide(Arc::new(Cache { tenant: "a" }))?;
    /// tenant_b.provide(Arc::new(Cache { tenant: "b" }))?;
    ///
    /// assert_eq!(tenant_a.try_service::<Cache>()?.tenant, "a");
    /// assert_eq!(tenant_b.try_service::<Cache>()?.tenant, "b");
    /// // The default realm never saw a provider — isolation cuts both ways.
    /// assert!(ctx.try_service::<Cache>().is_err());
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// This is the direct fresh-private operation: every call chooses a new
    /// realm and changes only the isolate axis. To join an exact Service slot,
    /// allocate and reuse a realm through [`Context::with_service_realms`].
    #[must_use = "a derived context that is dropped has no effect"]
    pub fn with_isolated_service(&self, name: &str) -> Self {
        let mut map = HashMap::new();
        map.insert(name.to_owned(), self.root.sn.next_realm_key());
        self.derive_isolate(map)
    }

    /// Derive a Context mapping several Service names into exact realms in
    /// one atomic isolate layer; lookup shadows nearest-layer-first exactly like
    /// [`Context::with_isolated_service`], and names absent from `realms`
    /// retain the source view's mapping or the Runtime default.
    ///
    /// The complete batch is validated before a Context is derived. Duplicate
    /// Service names are rejected before foreign-Runtime membership is checked;
    /// either failure leaves the source Context unchanged.
    #[must_use = "a derived context that is dropped has no effect"]
    pub fn with_service_realms<I, K>(
        &self,
        realms: I,
    ) -> std::result::Result<Self, crate::service::RealmMappingError>
    where
        I: IntoIterator<Item = (K, crate::ServiceRealm)>,
        K: Into<String>,
    {
        // Conversion is user code. Complete it before validation so a panic or
        // reentrant call cannot leave a partial layer behind.
        let pairs: Vec<(String, crate::ServiceRealm)> = realms
            .into_iter()
            .map(|(name, realm)| (name.into(), realm))
            .collect();

        let mut names = HashSet::with_capacity(pairs.len());
        for (name, _) in &pairs {
            if !names.insert(name.clone()) {
                return Err(crate::service::RealmMappingError::DuplicateService {
                    service: name.clone(),
                });
            }
        }

        for (name, realm) in &pairs {
            if !realm.belongs_to(&self.root.realm_membership) {
                return Err(crate::service::RealmMappingError::ForeignRealm {
                    service: name.clone(),
                });
            }
        }

        let map = pairs
            .into_iter()
            .map(|(name, realm)| (name, realm.key()))
            .collect();
        Ok(self.derive_isolate(map))
    }

    /// Shared derive tail for isolate-family derivations: advance only the
    /// isolate position while preserving current Fiber, Scope, and intercept.
    fn derive_isolate(&self, map: HashMap<String, RealmKey>) -> Self {
        Self {
            root: self.root.clone(),
            fiber: self.fiber.clone(),
            isolate: Some(Arc::new(IsolateLayer {
                map,
                outer: self.isolate.clone(),
            })),
            scope: self.scope.clone(),
            intercept: self.intercept.clone(),
        }
    }

    /// Return this Context's opaque Runtime-local Event routing position.
    #[must_use]
    pub fn scope(&self) -> Scope {
        Scope {
            membership: self.root.scope_membership.clone(),
            layer: self.scope.clone(),
        }
    }

    /// Derive one child Event [`Scope`] while preserving this Context's
    /// Fiber, isolate, and intercept positions exactly.
    ///
    /// A dispatch routed to the child's scope can reach registrations on
    /// this Context's scope and the child itself, but not registrations on
    /// sibling or descendant scopes unless they are explicitly global.
    #[must_use = "a derived context that is dropped has no effect"]
    pub fn with_child_scope(&self) -> Self {
        Self {
            root: self.root.clone(),
            fiber: self.fiber.clone(),
            isolate: self.isolate.clone(),
            scope: Some(ScopeNode::derive(self.scope.as_ref())),
            intercept: self.intercept.clone(),
        }
    }

    /// Derive a Context with one new innermost prepared layer for Service `S`.
    ///
    /// This changes only the intercept axis and performs no source preparation
    /// or lifecycle work.
    #[must_use = "a derived Context that is dropped has no effect"]
    pub fn with_intercept<S: crate::ConfigurableService>(&self, layer: S::Layer) -> Context {
        self.with_configured_intercept(
            S::NAME.to_owned(),
            crate::plugin::ConfiguredLayer::new::<S, _>(layer),
        )
    }

    pub(crate) fn with_configured_intercept(
        &self,
        name: String,
        configured: crate::plugin::ConfiguredLayer,
    ) -> Context {
        Self {
            root: self.root.clone(),
            fiber: self.fiber.clone(),
            isolate: self.isolate.clone(),
            intercept: Some(Arc::new(InterceptLayer {
                name,
                configured,
                outer: self.intercept.clone(),
            })),
            scope: self.scope.clone(),
        }
    }

    /// Resolve a ConfigurableService from an optional base, every matching
    /// intercept layer in outer-to-inner order, and an optional operation head.
    ///
    /// Layer collection and Service-owned composition run synchronously with
    /// no framework lock held.
    pub fn resolve_config<S: crate::ConfigurableService>(
        &self,
        base: Option<&S::Layer>,
        head: Option<&S::Layer>,
    ) -> std::result::Result<S::Resolved, crate::service::ConfigResolutionError<S::ComposeError>>
    {
        let mut configured = Vec::new();
        let mut node = self.intercept.as_deref();
        while let Some(current) = node {
            if current.name == S::NAME {
                configured.push(current.configured.clone());
            }
            node = current.outer.as_deref();
        }
        configured.reverse();

        if configured.iter().any(|layer| !layer.matches::<S>()) {
            return Err(crate::service::ConfigResolutionError::ContractMismatch {
                service: S::NAME,
            });
        }
        let layers = configured.iter().map(|layer| {
            layer
                .downcast_ref::<S::Layer>()
                .expect("matching Service contracts retain their declared Layer type")
        });
        S::compose_config(base, layers, head)
            .map_err(crate::service::ConfigResolutionError::Compose)
    }
}

/// Unforgeable membership shared by every Scope capability of one Runtime.
#[derive(Debug)]
pub(crate) struct ScopeMembership;

/// Opaque Runtime-local Event routing position.
#[derive(Clone)]
pub struct Scope {
    pub(crate) membership: Arc<ScopeMembership>,
    pub(crate) layer: Option<Arc<ScopeNode>>,
}

impl Scope {
    pub(crate) fn belongs_to(&self, membership: &Arc<ScopeMembership>) -> bool {
        Arc::ptr_eq(&self.membership, membership)
    }
}

impl std::fmt::Debug for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Scope(..)")
    }
}

/// One node of the Scope chain. Only [`Context::with_child_scope`] appends
/// a node; isolate and intercept derivations preserve the chain unchanged.
/// `new()` and `root()` start at the root Scope (`None`). Scoped dispatch
/// walks this chain by pointer identity.
#[derive(Debug)]
pub(crate) struct ScopeNode {
    parent: Option<Arc<ScopeNode>>,
}

impl ScopeNode {
    /// Whether `head` (a hook's registration-time scope) is reachable from
    /// this chain, i.e. the hook was registered on an ancestor-or-self
    /// scope of this node.
    pub(crate) fn reaches(self: &Arc<Self>, head: &Arc<ScopeNode>) -> bool {
        let mut node: Option<&Arc<ScopeNode>> = Some(self);
        while let Some(current) = node {
            if Arc::ptr_eq(current, head) {
                return true;
            }
            node = current.parent.as_ref();
        }
        false
    }

    fn derive(parent: Option<&Arc<ScopeNode>>) -> Arc<ScopeNode> {
        Arc::new(ScopeNode {
            parent: parent.cloned(),
        })
    }
}

/// Persistent representation of one isolate position. `outer` belongs only to
/// this axis and is folded nearest-first; it is not Context ancestry and carries
/// no Scope, lifecycle, authorization, or Service-provider fallback meaning.
#[derive(Debug)]
pub(crate) struct IsolateLayer {
    map: HashMap<String, RealmKey>,
    outer: Option<Arc<IsolateLayer>>,
}

impl IsolateLayer {
    /// Nearest-layer-wins lookup; `None` when no layer defines `name`.
    fn lookup(&self, key: &str) -> Option<RealmKey> {
        let mut node = Some(self);
        while let Some(current) = node {
            if let Some(id) = current.map.get(key) {
                return Some(*id);
            }
            node = current.outer.as_deref();
        }
        None
    }
}

/// One prepared Service configuration layer plus the next outer intercept
/// layer. This chain represents only the ordered intercept axis; it is not a
/// generic Context hierarchy.
pub(crate) struct InterceptLayer {
    name: String,
    configured: crate::plugin::ConfiguredLayer,
    outer: Option<Arc<InterceptLayer>>,
}

#[cfg(test)]
mod tests {
    use super::SnCounters;
    use std::sync::atomic::Ordering;

    #[test]
    fn realm_key_exhaustion_refuses_to_reuse_an_identity() {
        let counters = SnCounters::default();
        counters.realm.store(u64::MAX, Ordering::Relaxed);

        assert!(
            std::panic::catch_unwind(|| counters.next_realm_key()).is_err(),
            "exhaustion must stop allocation before the default or an existing realm can repeat"
        );
    }
}

#[cfg(test)]
mod issue26_tests {
    use super::*;
    use crate::fiber::DependencyEdge;
    use crate::registry::PluginKey;
    use crate::{InjectSpec, Plugin, PreparedPlugin, Service};
    use std::convert::Infallible;
    use std::future::Future;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FallbackService;
    impl Service for FallbackService {
        const NAME: &'static str = "issue26-fallback-service";
    }

    struct FallbackDependent(Arc<AtomicUsize>);
    impl Plugin for FallbackDependent {
        type Config = ();
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = Infallible;

        fn inject(&self) -> InjectSpec {
            InjectSpec::none().require(FallbackService::NAME)
        }

        fn prepare(&self, (): ()) -> Result<(), Infallible> {
            Ok(())
        }

        fn apply(
            &self,
            _ctx: Context,
            _prepared: &(),
        ) -> impl Future<Output = Result<(), Infallible>> + Send {
            self.0.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Ok(()))
        }
    }

    #[test]
    fn incomplete_dependency_projection_falls_back_to_fiber_owned_edges() {
        let ctx = Context::new();
        let fiber = Fiber::new_with_edges(
            "dependent",
            vec![DependencyEdge::new("svc".to_owned(), RealmKey::DEFAULT)],
        );
        ctx.root
            .registry
            .attach_fiber(PluginKey::Anonymous(26), fiber.clone());
        ctx.root.deps.register(&fiber);
        ctx.root.deps.disable_for_test();

        let affected = ctx
            .root
            .dependents_for_edges(&[("svc".to_owned(), RealmKey::DEFAULT)]);
        assert_eq!(affected.len(), 1);
        assert_eq!(affected[0].id(), fiber.id());

        let residents = ctx.root.registry.snapshot_fibers();
        ctx.root.deps.rebuild_for_test(&residents);
        let rebuilt = ctx
            .root
            .dependents_for_edges(&[("svc".to_owned(), RealmKey::DEFAULT)]);
        assert_eq!(rebuilt.len(), 1);
        assert_eq!(rebuilt[0].id(), fiber.id());
    }

    #[tokio::test]
    async fn disabled_projection_preserves_service_settlement() {
        let ctx = Context::new();
        let applies = Arc::new(AtomicUsize::new(0));
        let dependent = ctx
            .spawn(PreparedPlugin::from_input(
                FallbackDependent(applies.clone()),
                (),
            ))
            .await
            .unwrap();
        assert_eq!(dependent.state(), crate::FiberState::Pending);

        ctx.root.deps.disable_for_test();
        let _publication = ctx.provide(Arc::new(FallbackService)).unwrap();

        assert_eq!(dependent.ready().await.unwrap(), crate::FiberState::Active);
        assert_eq!(applies.load(Ordering::SeqCst), 1);
    }
}

#[cfg(test)]
mod critical_section_tests {
    use super::Context;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct ReentrantRealmName {
        ctx: Context,
        converted: Arc<AtomicUsize>,
    }

    impl From<ReentrantRealmName> for String {
        fn from(value: ReentrantRealmName) -> Self {
            let snapshot = value.ctx.runtime_snapshot();
            assert!(!snapshot.fibers().is_empty());
            value.converted.fetch_add(1, Ordering::SeqCst);
            "issue55/conversion".to_owned()
        }
    }

    struct ReentrantConfig;
    impl crate::Service for ReentrantConfig {
        const NAME: &'static str = "issue55/config-compose";
    }
    impl crate::ConfigurableService for ReentrantConfig {
        type Config = Context;
        type Layer = Context;
        type Resolved = ();
        type PrepareError = std::convert::Infallible;
        type ComposeError = std::convert::Infallible;

        fn prepare_config(config: Self::Config) -> Result<Self::Layer, Self::PrepareError> {
            assert!(!config.runtime_snapshot().fibers().is_empty());
            Ok(config)
        }

        fn compose_config<'a>(
            base: Option<&'a Self::Layer>,
            layers: impl IntoIterator<Item = &'a Self::Layer>,
            head: Option<&'a Self::Layer>,
        ) -> Result<Self::Resolved, Self::ComposeError> {
            for ctx in base.into_iter().chain(layers).chain(head) {
                assert!(!ctx.runtime_snapshot().fibers().is_empty());
            }
            Ok(())
        }
    }

    #[test]
    fn configuration_prepare_and_compose_can_reenter_runtime_observation() {
        use crate::ConfigurableService;
        let ctx = Context::new();
        let prepared = ReentrantConfig::prepare_config(ctx.clone()).unwrap();
        ctx.resolve_config::<ReentrantConfig>(Some(&prepared), None)
            .unwrap();
    }

    #[test]
    fn realm_name_conversion_can_reenter_runtime_observation() {
        let ctx = Context::new();
        let converted = Arc::new(AtomicUsize::new(0));
        let realm = ctx.new_service_realm();
        let mapped = ctx
            .with_service_realms([(
                ReentrantRealmName {
                    ctx: ctx.clone(),
                    converted: converted.clone(),
                },
                realm,
            )])
            .unwrap();
        assert_eq!(converted.load(Ordering::SeqCst), 1);
        drop(mapped);
    }
}
