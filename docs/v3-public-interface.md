# Cordis v3 public interface

This document is the exhaustive caller-visible contract of the three
Cordis v3 semantic crates — `cordis-core`, `cordis-loader`, and
`cordis-timer` — plus the supported `cordis-rs` application facade.
Every public module path, crate-root re-export, declaration, trait bound,
enum variant, accessor, operation result, failure phase, and cancellation
law of the approved v3 interface appears here exactly once, at its
canonical path. It is normative: an implementation or a downstream
consumer that contradicts it is wrong.

Each companion document owns one concern and never restates these
declarations: [docs/v3-architecture.md](v3-architecture.md) is the sole
normative entry point linking this set; [CONTEXT.md](../CONTEXT.md)
defines the domain vocabulary used below;
[ADR 0036](adr/0036-module-and-crate-seams-are-semantic.md)
fixes the semantic module and crate seams and
[ADR 0037](adr/0037-public-interfaces-expose-semantics-not-representation.md)
the interface policy these declarations follow; the exact-pin upstream
parity ledger records upstream relationships; the non-normative migration
inventory records compatibility consequences for the previous interface.

Signatures below are the contract: `Context::name` denotes an inherent
method on `Context`, and a type named without a module is reachable as
the facades section states.

## 1.0 API freeze candidate

This inventory is the **candidate 1.0 supported surface**. The freeze baseline
is the first repository revision that contains this declaration together with
the completed `Declare an API freeze candidate` criterion in `ROADMAP.md`;
releases that predate that revision do not count as post-freeze stabilization
evidence. The audit supporting this declaration is recorded in
[`api-freeze-candidate.md`](api-freeze-candidate.md).

The freeze applies only to the supported compatibility surface defined by
[`compatibility-policy.md`](compatibility-policy.md): these declarations and
caller-visible semantics, the documented supported Cargo surface, the
`cordis-rs` facade, and the published-sibling compatibility obligation. It does
not freeze private representation, doc-hidden unsupported downstream surface,
test facilities, or other implementation details into the public contract.

Compatible additive API and implementation work may continue after this point.
Correctness and security fixes remain mandatory; a fix that preserves this
contract does not disturb the freeze. If a deliberate change instead breaks the
candidate supported contract, or a correctness/security finding proves that the
candidate contract itself must change incompatibly, update the owning authority,
land the break, and restart the ROADMAP stabilization-release count from that
new baseline. Cleanup, refactoring, and documentation-only changes do not
restart that count when they preserve the supported contract.

## Interface promises

The interface guarantees, for every item in every crate:

- **No compatibility alias.** A removed or renamed path fails to
  compile; no old and new spelling coexist to ease migration.
- **No second canonical semantic path.** Within each semantic crate, each
  specialist item is reachable at exactly one semantic module path. The
  crate-root whitelists below are the only convenience re-exports, and
  there is no glob re-export. The explicitly listed `cordis-rs`
  application facade is a supported mirror of the `cordis-core` facade,
  not a second declaration of those semantics.
- **No broad prelude.** No crate ships a prelude module or any facade
  beyond the explicit root whitelists.
- **No storage escape.** No public item exposes storage topology,
  backing collections, raw handles, claim machinery, or partial
  transaction phases. Mutation authority over one occurrence travels
  only in the move-only capability returned by the operation that
  created the occurrence.
- **No downcast.** No public contract exposes `Any`, requires a
  downcast, or returns an original heterogeneous error object.
  Normalized failures expose only their semantic kind, owned diagnostic
  text, and justified correlation identities.
- **No extra trait bound.** Every bound on a public contract is required
  by that operation's own semantics. There is no universal `Default`,
  `Clone`, `Send`, `Sync`, or `'static` bound beyond the declarations
  below.

## Authoring style and macro surface

The intended 1.0 surface intentionally has **no supported public macro surface**.
`cordis-core`, `cordis-loader`, `cordis-timer`, and the `cordis-rs` facade
export no declarative macro, derive macro, attribute macro, or function-like
procedural macro as part of the downstream contract.

The canonical authoring style is ordinary Rust declarations plus direct
implementation of the semantic traits, including `Plugin`, `Service`,
`ConfigurableService`, `Event`, and `PluginResolver`. Listener roles are
constructed through the ordinary public adapter functions rather than generated
syntax.

This is deliberate rather than an unfinished convenience layer. The small
`Service` and `Event` implementations make semantic identity and typed contract
shape explicit. The longer `Plugin` and `ConfigurableService` implementations
contain preparation, application, configuration-composition, error, and bound
semantics that a macro would not remove. A macro that merely rewrites those
implementations would add a second supported authoring syntax without reducing
the semantic decisions a consumer must make; a macro that hides them would make
the contract less explicit.

A future macro may be introduced additively when demonstrated consumer usage
shows a repeated authoring pattern with one unambiguous semantic meaning. Its
documented invocation syntax and caller-visible generated API or behavior then
join the supported surface. Generated helper names and internal expansion
structure remain implementation details unless the normative interface
explicitly exposes them.

## Crate facades and canonical paths

Rust modules are private by default. Each crate exposes one curated
semantic facade; the public modules are exactly the ones named here.

### cordis-core

The supported downstream contract is the default-feature surface described
below. The non-default `internal-api` Cargo feature is a doc-hidden
published-sibling implementation seam used by `cordis-timer` and
`cordis-loader`; it is not supported downstream API. Cargo feature unification
can nevertheless enable it for a consumer that also depends directly on
`cordis-core`, making `cordis_core::__internal` reachable. Reachability does not
grant a downstream compatibility promise, and `cordis-rs` never re-exports
those details. The separate obligation to keep already-published sibling crates
compiling across every core version admitted by their dependency requirements
is defined by [the compatibility policy](compatibility-policy.md).

`cordis-core` exposes exactly these supported public semantic modules:

```text
plugin  lifecycle  service  event  effect  logger  observation
```

Its crate-root re-export whitelist is exactly:

```text
Context
Plugin  PreparedPlugin  PreparedChange  InjectSpec
FiberHandle  FiberId  FiberState  UpdateOutcome
Service  ConfigurableService  ServiceRealm
Event  Scope  Routing  QueryOutcome
Logger  Level  BoxError
```

The Event module is singular `event`. `Context` and `BoxError` are
crate-root-only facade items. Every other specialist item has exactly
one canonical semantic module path:

| Core module | Canonical specialist items |
| --- | --- |
| `plugin` | `Plugin`, `PreparedPlugin`, `PreparedChange`, `InjectSpec` |
| `lifecycle` | `FiberHandle`, `FiberId`, `FiberState`, `FiberRole`, `UpdateOutcome`, `PluginFailure`, `PluginFailureKind`, `LifecycleRecursion`, `LifecycleOperation`, `SpawnError`, `ReadyError`, `RestartError`, `WaitStateError`, `UpdateError`, `EraSwapError`, `EraSwapFailure`, `UpdateListener`, `UpdateNext` |
| `service` | `Service`, `ConfigurableService`, `ServiceRealm`, `ServicePublication`, `RealmMappingError`, `ServiceLookupError`, `ServicePublishError`, `ServiceControlError`, `ConfigResolutionError` |
| `event` | `Event`, `Scope`, `Routing`, `QueryOutcome`, `Listener`, `ListenerOptions`, `ListenerRegistration`, `ListenerRegistrationId`, `Next`, `StatefulCallback`, `observer`, `observer_sync`, `responder`, `responder_sync`, `mapper`, `mapper_sync`, `around`, `with_state`, `ListenerRole`, `EventOperation`, `DispatchOutcomeKind`, `InvocationFailure`, `InvocationFailureKind`, `ParallelFailures`, `ListenerRegistrationError`, `DispatchError` |
| `effect` | `CleanupResult`, `EffectRegistration`, `EffectRegistrationError`, `EffectFailure`, `EffectFailureKind`, `TaskRegistrationError` |
| `logger` | `Level`, `LogRecord`, `Logger`, `Exporter`, `ExporterRegistration`, `BufferExporter`, `BufferSizeZero` |
| `observation` | `RuntimeSnapshot`, `FiberSnapshot`, `ServiceSnapshot`, `ServicePublicationId`, `ScopeId`, `ObservationRouting`, `RuntimeObservation`, `ResidencyChange`, `ListenerChange`, `RuntimeObserver` |

The facade has no further public paths:

- The Context-view machinery, the Fiber internals, and the Registry are
  implementation-only; their caller-visible capabilities live in
  `Context` and the modules above, and Registry topology never escapes.
- There is no public `error` module: every error type lives in the
  module of the operation that can produce it.
- There is no supported public `internal` module: observation and control
  transport is private. The feature-gated `__internal` workspace seam described
  above is explicitly outside the downstream contract.
- There is no `list` module: the capability is absent from the baseline.

### cordis-loader

`cordis-loader` exposes exactly the semantic modules `plan`, `resolver`,
and `outcome`. Its crate-root whitelist is `EntryId`, `PluginEntry`,
`EntryGroup`, `LoadPlanBuilder`, `LoadPlan`, `PluginResolver`, and
`LoadOutcome`.

| Loader module | Canonical specialist items |
| --- | --- |
| `plan` | `EntryId`, `PluginEntry`, `EntryGroup`, `InjectEntry`, `IsolateEntry`, `RealmPolicy`, `LoadPlanBuilder`, `LoadPlan`, `PlanError` |
| `resolver` | `PluginResolver`, `PluginRequest`, `ResolverFailure`, `ResolverFailureKind`, `JsonPrepareError`, `prepare_plugin_json`, `prepare_service_json` |
| `outcome` | `EntryOutcome`, `LoadOutcome`, `LoaderFailure` |

### cordis-timer

`cordis-timer` is an optional flat leaf whose root contains exactly
`TimerExt`, `Sleep`, `Timeout`, `TimeoutOutcome`, `Interval`,
`TimerCancelled`, and `TimerRegistrationError`.

### cordis-rs application facade

`cordis-rs` preserves the application import name `cordis` and mirrors the
supported `cordis-core` facade. Its root whitelist is the `cordis-core`
root whitelist above. It also re-exports the seven core semantic modules
`effect`, `event`, `lifecycle`, `logger`, `observation`, `plugin`, and
`service`. It does not re-export Loader, Timer, or sibling-only
internals. Mirrored items retain their `cordis-core` contracts and
introduce no independent semantics.

## Context and its axes

`Context::new()` is the sole ordinary Runtime constructor and returns
the root Context. `Context` has no `Default`. `Clone` duplicates the
exact view. `root()` is `must_use` and resets current-Fiber attribution
plus the isolate, Scope, and intercept axes to their roots in the same
Runtime.

Context derivations are axis-pure: each changes only its named axis and
never starts lifecycle work.

```text
with_isolated_service(name)       isolate only; fresh private realm
new_service_realm()               allocate opaque Runtime-local realm
with_service_realms(mappings)     isolate only; atomic validated batch
with_child_scope()                Scope only
scope()                           portable Event-routing position
with_intercept::<S>(S::Layer)     intercept only; new innermost layer
```

`with_service_realms` rejects duplicate Service names before
foreign-realm membership and installs nothing on failure.

`ServiceRealm` is opaque, Runtime-local, and non-constructible, with
`Debug + Clone + Eq + Hash` and nothing more: it is not `Copy`, not
ordered, not displayed, not serialized, and never compares equal across
Runtimes. There is no `IsolateId`, no numeric or textual realm identity
in core, no global sentinel realm, no `IsolateRealm`, and no core
`Private`/`Shared` policy type.

`Scope` is an opaque `Debug + Clone` routing capability containing only
a Runtime association and one Event reachability position. It exposes no
ancestry and no Context capabilities. `Routing::{Unscoped,
Scoped(Scope)}` is explicit on each Event primitive. A foreign Scope
fails before selection or claim.

Context installs already-prepared intercept layers and resolves complete
Service-specific configuration; it exposes no raw layer storage and
prepares no source configuration during derivation. The raw intercept
getter is private.

`Context::spawn_attributed(future)` is the user-owned Tokio task seam for
apply/cleanup work that crosses a task boundary but remains part of the current
settle dependency graph. It carries exact-allocation lifecycle-recursion
attribution into the spawned task and returns the ordinary Tokio `JoinHandle`;
it does not make the task generation-owned. Raw `tokio::spawn` carries no such
attribution. Inherited frames expire with their source settle scope, so a task
that outlives that scope does not retain a stale recursion refusal.

## Plugin preparation, sealing, and creation

The base Plugin contract is:

```rust
pub trait Plugin: Send + 'static {
    type Config;
    type Input: Send + 'static;
    type PrepareError: std::error::Error;
    type ApplyError: std::error::Error;

    fn name(&self) -> std::borrow::Cow<'_, str>;
    fn inject(&self) -> InjectSpec;
    fn prepare(
        &self,
        config: Self::Config,
    ) -> Result<Self::Input, Self::PrepareError>;
    fn apply(
        &self,
        ctx: Context,
        input: &Self::Input,
    ) -> impl std::future::Future<
        Output = Result<(), Self::ApplyError>,
    > + Send;
}
```

`Config` has no universal `Default`, `Clone`, `Send`, `Sync`, or
`'static` bound. `Input` is `Send + 'static` but need not be `Clone`
or `Sync`. `PrepareError` and `ApplyError` require only
`std::error::Error`; they normalize only at a later boundary that
actually retains or erases them. `Plugin` has no `Sync` supertrait.
`impl Plugin for Arc<P>` exists only for `P: Plugin + Sync` and
delegates the complete contract. `name()` is diagnostic only and
defaults to `type_name`; `inject()` defaults to `InjectSpec::none()`.
There is no declaration-only `Plugin::provide`.

`PrepareError` and `ApplyError` are concrete Plugin-authoring error types, not
application-erasure slots. `BoxError` remains an optional outer application boundary
such as `main -> Result<(), BoxError>`; it is not the canonical Plugin associated error.
When apply code uses `?` across several operation-specific families, define an
application-local concrete error enum or newtype with the required conversions so those
boundaries remain explicit until the Plugin apply boundary normalizes the final error.

Creation crosses three explicit boundaries:

```rust
let input = plugin.prepare(config)?;
let target = PreparedPlugin::from_input(plugin, input);
let fiber_handle = ctx.spawn(target).await?;
```

`PreparedPlugin` is opaque, move-only, and `must_use`, with one public
constructor: `from_input<P: Plugin>(P, P::Input)`. It performs
only typed association sealing. A consuming
`with_inject_overlay(InjectSpec)` completes a Loader dependency overlay
before spawn without changing Plugin contract identity. `name()` and
`inject()` are materialized during sealing and are never called by
Runtime lifecycle paths. Direct panics in these synchronous typed
contracts unwind normally, under no framework lock and before lifecycle
admission.

`Context::spawn(PreparedPlugin)` is the whole creation transaction. It
returns a `FiberHandle` only after a fresh Fiber reaches live quiescent
`Active` or stable `Pending`. Initial apply failure leaves no resident
Fiber. Cancellation law: before the allocation/publication commit,
cancelling the spawn future has no lifecycle effect; afterward the
framework completes the FiberHandle handoff or fully disposes and unlinks the
undelivered Fiber independently of caller polling.

Absent from the interface: public `DynPlugin`, raw `ErasedConfig`,
`InterceptConfig`, `any_plugin`, erased methods, and downcast/storage
hooks are private; `PluginBuilder`, `Context::plugin`, config/default
conveniences, and builder spawn do not exist. A convenience cannot
conceal the preparation ownership/error boundary or the lifecycle
commit boundary.

## Dependency declarations and Service configuration

`InjectSpec` is opaque, `must_use`, and `Clone + Debug + Default`, with
`none()` as the semantic empty declaration. It has two consuming
upserts:

```text
require(name)                       required, no configured layer
require_configured::<S>(S::Layer)   required with prepared typed layer
```

One effective declaration exists per Service. A later upsert replaces an
earlier one, and `require` explicitly clears configured metadata. Final
entry order is nonsemantic. `Clone` shares immutable private layers and
does not require a Layer to be `Clone`. There is no `new`, no batch
`requires`, no raw `require_with`, no `require_overriding`, and no
public iteration, emptiness query, field, or declaration-order access.

`Service` is the named typed value contract: `Send + Sync + 'static`
with a `NAME`. Configuration is opt-in:

```rust
pub trait ConfigurableService: Service {
    type Config;
    type Layer: Send + Sync + 'static;
    type Resolved: Send + 'static;
    type PrepareError: std::error::Error;
    type ComposeError: std::error::Error;

    fn prepare_config(
        config: Self::Config,
    ) -> Result<Self::Layer, Self::PrepareError>;

    fn compose_config<'a>(
        base: Option<&'a Self::Layer>,
        layers: impl IntoIterator<Item = &'a Self::Layer>,
        head: Option<&'a Self::Layer>,
    ) -> Result<Self::Resolved, Self::ComposeError>;
}
```

Source configuration, retained layer, and resolved configuration stay
distinct. Context owns which base, outer-to-inner layers, and head are
supplied; the Service contract owns composition meaning. There is no
universal merge, default, `Clone`, or extra transport bound. Preparation
and composition are synchronous and Context-free. `with_intercept`
accepts only a prepared Layer; no Config-taking convenience exists.
`resolve_config::<S>` returns the typed `Resolved` value or
`ConfigResolutionError<S::ComposeError>`.

## Service publication and lookup

`Context::try_service::<S>()` is exact visible lookup in the
Context-selected realm and fails with `ServiceLookupError`.
`Context::provide::<S>(Arc<S>)` is fallible with `ServicePublishError`
and on success returns an opaque, non-Clone, non-Copy, `must_use`
`ServicePublication<S>`. There is no Context-wide `set_service` or
`remove_service`. The capability controls exactly one publication
occurrence:

```text
set(&self, Arc<S>) -> Result<(), ServiceControlError>
remove(self)       -> Result<(), ServiceControlError>
```

`set` changes only the same occurrence's payload and causes no
dependency-target drift. `remove` competes at most once with generation
cleanup; late stale cleanup cannot affect a replacement. Closing blocks
new caller mutation. `ServiceControlError` distinguishes
`StalePublication` from a current `MutationClosed` occurrence, checking
stale identity first. Dropping the capability is inert; generation
cleanup remains the publication owner. Mutation authority follows the
occurrence that created it and is never rediscovered from Context,
Service name, or slot state.

## Fiber identity, lifecycle, and typed group removal

`FiberState` has exactly the variants `Loading`, `Active`, `Pending`,
`Unloading`, `Failed`, and `Disposed`, with `Debug + Clone + Copy + Eq`;
it has no default, numeric representation, ordering, or serde contract.

`FiberHandle` is opaque, `Clone + Debug`, and inert on Drop. Its accessors are
`name(&self) -> &str`, `state(&self) -> FiberState`, `id(&self) -> FiberId`, and
`pending_missing(&self) -> Vec<String>`.

All lifecycle controls are async: `ready(&self) -> Result<FiberState, ReadyError>`;
`wait_state(&self, state: FiberState, timeout: std::time::Duration)` returns
`Result<(), WaitStateError>`;
`restart(&self) -> Result<(), RestartError>`; `update(&self, change: PreparedChange)`
`-> Result<UpdateOutcome, UpdateError>`; `era_swap(&self, change: PreparedChange)`
`-> Result<FiberHandle, EraSwapError>`; and `dispose(&self)`
`-> Result<(), LifecycleRecursion>`.

`FiberId` is opaque Runtime-local correlation identity with `Debug + Clone + Eq + Hash`
only; there is no nullable numeric `uid()`. Restart, same-Fiber update, and disposal
preserve it; era replacement allocates a new identity; cross-Runtime identities never
compare equal. It grants no lookup or control.

`PreparedChange::from_input::<P>(P::Input)` seals a move-only,
`must_use`, one-attempt candidate associated with Plugin contract `P`.
It carries no Plugin behavior or lifecycle authority. Both lifecycle
operations consume it:

```text
update(change)   -> same Fiber/FiberId; new generation
era_swap(change) -> old Fiber ends; fresh Fiber/FiberId
```

`update` returns `UpdateOutcome::{Committed(FiberState), Vetoed}`. A
committed state is only `Active` or stable `Pending`. `Vetoed` is normal
precommit policy refusal and never revives the consumed candidate.
Postcommit apply failure retains the new authoritative Prepared and
parks the target as `Failed`. `UpdateOutcome` is
`Debug + Clone + Copy + Eq` with no `Default`.

Era swap completes compatibility, liveness, and recursion preflight
before its single irreversible source claim. Success requires old-Fiber
disposal, a live quiescent successor, fresh dependent queries and final
convergence, and FiberHandle handoff. `EraSwapError::Incomplete` means the old
Fiber is gone, no attempted successor remains resident, and final
cleanup and convergence completed; it reports successor-specific causes
and never embeds `SpawnError`.

The Registry has no public presence: there is no public `Registry`, no
`Runtime`/`RuntimeId`/record types, no `PluginKey` or PluginGroup, no
row fields, counters, or detach state, and no `Context::registry()`.
The only grouping administration operation is:

```rust
pub async fn Context::remove_plugins<P: Plugin>(
    &self,
) -> Result<(), LifecycleRecursion>;
```

It selects the private typed grouping equivalence, atomically detaches
one current allocation, freezes its completion set, and awaits ordinary
disposal and unlink/prune for every member. Later same-type spawns use a
fresh allocation outside that removal. Absence is success. Self-wait
recursion is refused before detach. After detach, completion is
framework-owned and independent of caller cancellation.

## Runtime snapshots and observations

All Registry, Service, count, and pending observations collapse into
`Context::runtime_snapshot() -> RuntimeSnapshot`. Snapshot fields and
constructors are private; access is read-only:

```text
RuntimeSnapshot
    fibers()   -> &[FiberSnapshot]
    services() -> &[ServiceSnapshot]

FiberSnapshot
    id()               -> &FiberId
    role()             -> FiberRole
    name()             -> &str
    state()            -> FiberState
    missing_services() -> &[String]

ServiceSnapshot
    id()       -> &ServicePublicationId
    service()  -> &str
    realm()    -> &ServiceRealm
    provider() -> &FiberId
    visible()  -> bool
```

Each snapshot contains exactly one active `FiberRole::Root` record with
an empty missing-Services list, plus every resident ordinary Fiber,
including `Disposed` Fibers not yet unlinked. Service records include
each current occupied publication, including Loading/invisible ones, and
exclude closed-generation stale physical rows. Each record is
self-consistent; the combined collections are not a globally
linearizable instant and need not be referentially closed. Collections
have no semantic order. Snapshot and record types are `Debug + Clone`;
correlation identities — `ServicePublicationId`, `ScopeId`,
`ListenerRegistrationId` — are opaque Runtime-local
`Debug + Clone + Eq + Hash` values without control power.

Framework observation is subscription-only. The single immutable record
family is:

```rust
pub enum RuntimeObservation {
    FiberResidency {
        change: ResidencyChange,
        fiber: FiberSnapshot,
    },
    FiberState {
        fiber: FiberId,
        previous: FiberState,
        current: FiberState,
    },
    ServiceVisibility {
        service: String,
        realm: ServiceRealm,
        previous: Option<ServicePublicationId>,
        current: Option<ServicePublicationId>,
    },
    ListenerRegistration {
        change: ListenerChange,
        listener: ListenerRegistrationId,
        event: &'static str,
        role: ListenerRole,
        scope: ScopeId,
        options: ListenerOptions,
    },
    DispatchCompleted {
        operation: EventOperation,
        event: &'static str,
        routing: ObservationRouting,
        outcome: DispatchOutcomeKind,
    },
}
```

`Context::observe_runtime` accepts only the sealed `RuntimeObserver`
capability, implemented for the existing Observer adapters. Policy is
fixed: repeatable, Runtime-wide, detached, unscoped, parallel,
attempt-all. The registering Context supplies callback attribution and
generation ownership. There is no public observation Event marker and no
emit, query, or waterfall operation over observations. `ScopeId` and
`ObservationRouting` report correlation without granting a Scope.
Committed facts precede best-effort detached delivery; delivery failures
never alter the source operation, and delivery recursively suppresses
its own narration by private purpose rather than Event name.

The complete supporting variants are
`ResidencyChange::{Admitted, Removed}`,
`ListenerChange::{Registered, Unregistered}`,
`ListenerRole::{Observer, Responder, Mapper, Around}`,
`EventOperation::{Emit, EmitParallel, Query, Waterfall}`,
`DispatchOutcomeKind::{Completed, Answered, Missed, Failed}`, and
`ObservationRouting::{Unscoped, Scoped(ScopeId)}`. The small enums are
`Debug + Clone + Copy + Eq`; `RuntimeObservation` and the snapshot
records are `Debug + Clone`. `FiberRole::{Root, Ordinary}` is
`Debug + Clone + Copy + Eq` with no representation, ordering, display,
or serialization contract.

## Event contracts, roles, and dispatch

The base Event marker has no supertraits:

```rust
pub trait Event {
    const NAME: &'static str;
    type Args: Send + 'static;
    type Output: Send + 'static;
}
```

Event identity is semantic name plus a compatible Args/Output contract.
Private `TypeId` use may enforce the contract but never routes the
Event. There are no universal marker `Send`/`Sync`/`'static` bounds and
no payload `Clone`/`Sync` bounds. `Args: Clone` is imposed only by
`emit`, `emit_parallel`, `query`, and the derived `waterfall_query`;
`waterfall` transports one owned, possibly move-only Args. Output is
moved and never universally `Clone`.

Exactly four primitive operations exist, each taking explicit `Routing`:

```text
emit           ordered awaited notification; fail first
emit_parallel  claim/start all, await all, aggregate all failures
query          sequential first Answer, explicit Miss
waterfall      owned Mapper/Around onion with mandatory tail
```

There are no scoped twins and no serial, bail, or parallel aliases.
`QueryOutcome::{Miss, Answer(T)}` makes presence explicit: false, zero,
empty text, and empty collections are answers. `waterfall_query` is
derived: its tail runs `query`, hands the full query result or failure
to the resolver, and propagates the same Routing. It is not a fifth
primitive or a store.

`Listener<E>` is sealed and methodless; the registration/storage
protocol and wrapper types are private. The public semantic-role
adapters are:

```text
Observer   observer / observer_sync
Responder  responder / responder_sync
Mapper     mapper / mapper_sync
Around     around
```

Sync means immediate completion and may still return a typed error.
Around has no sync adapter because its full role may await the
consuming, single-use `Next<E>`. `with_state(factory, callback)` returns
an opaque `StatefulCallback` and composes with every role; it constructs
invocation-local state exactly once, after routing, role preflight, and
a successful claim, outside locks.

`ListenerOptions` is opaque `Debug + Clone + Copy + Default`; the
default is append, scoped, repeatable. Consuming `prepend`, `global`,
and `once` builders plus read-only `is_*` facts replace public fields.
Event registration entry points are `on<E, L>(&self, listener: L)` and
`on_with<E, L>(&self, listener: L, options: ListenerOptions)`.
Both return `Result<ListenerRegistration, ListenerRegistrationError>` with
`E: Event` and `L: Listener<E>`. There are no once twins. A successful registration returns a move-only
`ListenerRegistration` whose Drop is inert and whose consuming
`remove() -> bool` controls one exact occurrence. A once claim
unregisters before callback invocation; ordinary claimed work continues
if a later removal wins. Event operation futures themselves are caller-owned,
not lifecycle transactions with detached postcommit completion: cancellation
before an invocation claim leaves that occurrence untouched, while cancellation
after a once claim never restores the consumed occurrence. Cordis does not
promise that an in-flight Event callback finishes after its operation future is
cancelled; callers that require callback completion await the operation.

Absent or private: the closure-shape marker module, custom `Listener`
implementations, `Listener::register`, `EventCarrier`, global carrier
construction, a named around wrapper type, the old state wrappers, and
all public store/claim machinery.

## Typed update control

Update control is typed and precommit. Sealed, non-dispatchable
`UpdateListener<P>` and opaque consuming `UpdateNext<P>` replace any
erased update Event or downcast protocol.
`Context::on_update::<P>(listener, options)` accepts only compatible
`mapper`, `mapper_sync`, or `around` adapters; Observer and Responder
fail at the type boundary. It routes internally as Scoped to the target
Fiber.

The precommit pipeline may transform, veto, or fail a candidate. A
mandatory private tail provisionally captures the accepted typed
candidate without performing lifecycle commit. Callback success and tail
acceptance are separate facts: success without a surviving tail
candidate is `UpdateOutcome::Vetoed`. Lifecycle admission is revalidated
after awaited control and before commit. Update control never runs for
era swap and cannot observe or recover a postcommit apply failure.
Update control is publicly subscribable but framework-invokable only.

## Event errors

Registration and dispatch have separate phase errors:

```rust
#[non_exhaustive]
pub enum ListenerRegistrationError {
    InactiveContext,
    EventContractMismatch { event: &'static str },
}

#[non_exhaustive]
pub enum DispatchError {
    EventContractMismatch { event: &'static str },
    ForeignScope,
    IncompatibleRole {
        operation: EventOperation,
        role: ListenerRole,
    },
    Invocation(InvocationFailure),
    Parallel(ParallelFailures),
}
```

Preflight failures claim nothing. `InvocationFailure` is opaque: it
exposes an optional `ListenerRegistrationId`, an
`InvocationFailureKind::{ReturnedError, Panic}`, and owned diagnostic
text; a `None` registration identity denotes framework tail execution.
`ParallelFailures` is opaque, guaranteed non-empty, and read-only;
`emit_parallel` uses it whenever any claimed invocation fails, even one.
Failures are reported in effective listener order, never completion
order.

Callback error types require only `std::error::Error`. The last adapter
that knows the concrete error normalizes it exactly once to owned
Runtime-safe diagnostics; the original object, type identity, `Any`, and
downcast never escape. Panic containment includes future polling and
`with_state` factories. There are no public shape or payload mismatch
errors; contract and role failures are the semantic variants above, and
remaining representation checks are private invariants.

## Effects and tasks

`Context::effect` and `Context::effect_sync` register one
generation-owned, at-most-once cleanup closure. The closure is
`FnOnce + Send + 'static` — not `Fn`, `Sync`, or `Clone`; async cleanup
futures are `Send`. The sealed `CleanupResult` adapts `()` and
`Result<(), E>` for `E: std::error::Error`.

`EffectRegistration` is move-only with inert Drop and no `must_use`,
`Clone`, or `Debug`:

```text
disarm(self)        -> bool
dispose(self).await -> Result<bool, EffectFailure>
```

Manual control and the automatic generation drain compete for one exact
claim. A winning dispose transfers completion to framework ownership,
independent of caller polling. Async cleanup is first polled on a process-wide
Cordis completion runtime, so Tokio runtime-bound resources created by the
cleanup bind there rather than to the caller's runtime; synchronous cleanup
keeps lifecycle-executor ordering. Resources captured earlier from another
runtime retain that external runtime's lifetime. A live settle attribution at
manual-dispose claim time follows the detached cleanup task; ordinary external
manual disposal carries none. Synchronous cleanup is for short non-blocking
bookkeeping; blocking work should use async cleanup plus
`tokio::task::spawn_blocking` so it cannot occupy a shared completion worker. A
returned failure or panic permanently consumes the occurrence. Generation drain
closes admission, claims the remaining entries, and runs all cleanup in strict
sequential LIFO order,
continuing after failure. Callbacks, awaits, and user-controlled
destruction occur outside locks.

There is no `EffectMeta`, no labels, and no `Context::effects()`.
Effects do not grow acquire, producer, iterator, or nested-lifecycle
APIs. `Context::run` is synchronous task registration with signature
`fn run<F>(&self, task: F) -> Result<(), TaskRegistrationError>` and bound
`F: std::future::Future + Send + 'static`. Its wrapper consumes task output and retains
framework `()` only, so `F::Output` has no universal `Send` bound. Registration
fails before the task starts; callers do not await `run` itself.
`Context::spawn_attributed` is deliberately different: it carries current settle
attribution into user-owned work and returns its `JoinHandle`; it registers no
generation cleanup or join.

## Logger

`Logger` is `Clone + Debug`; `Context::logger`, `log`, the level
helpers, `name`, and `must_use` `with_name` make up the facility. Logger
is a Runtime foundation facility, not a Service. `LogRecord` is an
opaque immutable `Debug + Clone` record with read-only sequence,
`SystemTime` timestamp, channel, Level, and text accessors. Sequence
orders Runtime-local record assignment only; it is neither persistent
nor exporter callback order.

`Level::{Debug, Info, Warn, Error}` has
`Debug + Clone + Copy + Eq + Ord` and `as_str`; it has no numeric
discriminants, and severity explicitly orders
`Debug < Info < Warn < Error`. An `Exporter` receives records at or
above `min_level(channel).unwrap_or(default_level())`; a `None` channel
threshold means no channel override, not disabled. Logger snapshots
exporter occurrences, releases locks, then filters and invokes each
panic-contained exporter attempt-all.

There is no `ExporterId` and no Context-wide numeric removal.
`Context::add_exporter` returns a move-only `ExporterRegistration` whose
Drop is inert and whose consuming `remove() -> bool` controls one exact
generation-owned occurrence. Removal prevents future snapshots but
cannot revoke a registration already retained by an in-flight Logger
snapshot.

`BufferExporter` is an explicitly installed bounded adapter with
fallible nonzero-capacity construction (failure: `BufferSizeZero`), a
configured threshold, snapshot, and clear. It is not `Clone`, not
`Default`, not serde, not automatically installed, and not promised
`Debug`. Its storage and eviction use exporter receipt order, which may
differ from LogRecord sequence.

`List<T>` — its module, its `Clone` behavior, and every method — is
absent: no demonstrated consumer needs the capability, and no
replacement collection is promised.

## Loader plan and source schema

`EntryId` is opaque plan-lineage correlation identity with
`Debug + Clone + Eq + Hash`. It has no public constructor, number,
`Copy`, ordering, display, serde, path, or index. Identities returned
during builder mutation survive finish and LoadPlan clones;
independently rebuilding equivalent content creates a fresh plan lineage
and unequal identities.

```rust
pub struct LoadPlanBuilder { /* private */ }

#[derive(Clone)]
pub struct LoadPlan { /* shared immutable plan */ }

impl LoadPlanBuilder {
    pub fn new() -> Self;
    pub fn add_plugin(
        &mut self,
        parent: Option<&EntryId>,
        entry: PluginEntry,
    ) -> Result<EntryId, PlanError>;
    pub fn add_group(
        &mut self,
        parent: Option<&EntryId>,
        group: EntryGroup,
    ) -> Result<EntryId, PlanError>;
    pub fn finish(self) -> Result<LoadPlan, PlanError>;
}
```

Builder creation starts the plan lineage. Parent references must be
same-lineage, already admitted, and semantically parent-capable; each
add is failure-atomic. Finish validates and freezes without renumbering.
The plan exposes no mutable patch or removal and no backing entry,
children, iteration, or topology API; there is no public `EntryTree` or
other navigable plan storage. It may execute repeatedly: Entry
identity is plan-stable while lifecycle results are execution-specific.
Parent-before-descendants and declaration-order siblings define Loader
sequencing, never Fiber ownership.

The mutable serialized source schema:

```rust
pub struct PluginEntry {
    pub key: Option<String>,
    pub name: Option<String>,
    pub config: serde_json::Value,
    pub disabled: bool,
    pub inject: Vec<InjectEntry>,
    pub isolate: Vec<IsolateEntry>,
}

pub struct EntryGroup { pub name: String }

pub enum InjectEntry {
    Required(String),
    Configured { service: String, config: serde_json::Value },
}

pub struct IsolateEntry {
    pub service: String,
    pub policy: RealmPolicy,
}

pub enum RealmPolicy {
    Private,
    Shared { label: String },
}
```

These source-schema types are `Debug + Clone + Serialize + Deserialize`; enum wire
syntax is explicit and independent of Rust variant layout. `EntryGroup::name`
is human-readable source syntax only: structural group names are not retained by
the frozen plan or copied into execution outcomes. Plugin config
is required; unit is JSON null. `key.or(name)` chooses the resolve key;
neither field becomes Plugin, Fiber, or Registry identity. Duplicate
Service names within inject or within isolate are plan errors; one name
in both is valid because dependency declaration and realm placement are
orthogonal. A disabled Plugin entry prunes execution of its whole
descendant subtree without mutating the plan or deleting EntryIds;
failure does not prune. Groups are structural only.

Loader interprets `Private` and per-execution `Shared` labels into fresh
opaque core realms. Labels never enter core and never rendezvous across
executions. Raw JSON ends at target-specific resolver adapters and never
enters core lifecycle.

## Loader resolver, execution, and outcomes

`PluginResolver` is an intentionally externally implementable
synchronous Loader seam:

```rust
pub trait PluginResolver {
    type Error: std::error::Error;

    fn resolve(
        &self,
        request: PluginRequest<'_>,
    ) -> Result<Option<PreparedPlugin>, Self::Error>;
}
```

A closure blanket implementation exists. `PluginRequest` is opaque. Its borrowed
accessors are `resolve_key(&self) -> &str`, `config(&self) -> &serde_json::Value`, and
`inject(&self) -> &[InjectEntry]`; it exposes no entry identity, isolate policy,
Context, topology, or Runtime state. `Some` means recognized: Plugin and
configured-inject inputs fully typed and prepared, overlay complete, and
sealed. `None` means unknown key. `Err` means typed preparation or
resolution failure. Loader catches a resolver panic and normalizes it.
The resolver receives no placement policy; Loader independently derives
the Context realms before spawn. There is no async resolver.

The JSON helpers `prepare_plugin_json` and `prepare_service_json` are
typed Plugin and Service preparation helpers: they deserialize and
adapt, then call the corresponding typed preparation contract, returning
prepared semantic values — never `DynPlugin` or `Any` — and failing
with `JsonPrepareError`. `JsonPrepareError<E>::prepare_error() -> Option<&E>`
returns the concrete typed preparation failure without adding a `'static`
bound to `E`. For the same reason, the standard `Error::source()` chain can
expose a serde deserialization error but is intentionally `None` for an
arbitrary typed preparation error. Raw intercept injection does not exist.

`LoadPlan::load` is asynchronous with exact signature
`async fn load<R>(&self, ctx: &Context, resolver: &R) -> LoadOutcome`
where `R: PluginResolver + ?Sized`. It produces one ordered outcome per plan entry:

```rust
#[derive(Debug)]
pub enum EntryOutcome {
    Group { id: EntryId },
    Disabled { id: EntryId },
    Pruned { id: EntryId, disabled_ancestor: EntryId },
    Spawned { id: EntryId, resolve_key: String, fiber_handle: FiberHandle },
    Failed { id: EntryId, resolve_key: String, failure: LoaderFailure },
}
```

`EntryOutcome` is `Debug` and exposes `id(&self) -> &EntryId`. `LoadOutcome` is `Debug +
must_use` and exposes `entries(&self) -> &[EntryOutcome]`, `entry(&self, id: &EntryId)
-> Option<&EntryOutcome>`, `fiber_handles(&self) -> impl Iterator<Item = &FiberHandle>`,
and `is_ok(&self) -> bool`. Ignoring a delivered outcome would discard the caller's
FiberHandle controls while Fiber residency remains explicit. `Pruned` wins for every
descendant of a disabled Plugin, including
groups and separately disabled Plugins. Reachable groups yield `Group`;
reachable disabled Plugins yield `Disabled`. Ordinary resolver, preparation, or spawn failure yields `Failed` and does not
prune descendants. Loader realm placement has no independent execution failure:
duplicate isolate rows are rejected while building the plan, and every
execution realm is allocated from the caller Context's Runtime, so the private
validated mapping cannot produce `RealmMappingError`. `is_ok` is true exactly
when no `Failed` exists.
Resolve key is repeatable metadata; there is no `by_resolve_key` lookup.

Load is partial, not transactional. Before final outcome handoff, Loader
is responsible for either handing each already-delivered FiberHandle to the
caller or disposing it when the result cannot be delivered; abandonment
transfers reverse-success-order, attempt-all rollback to framework-owned
completion. Core owns an in-progress spawn until FiberHandle handoff; Loader
owns result handoff afterward; the caller owns the delivered outcome.
Ordinary entry failures do not trigger rollback. Dropping a delivered
LoadOutcome or FiberHandle is inert.

Plan construction fails with `PlanError`; execution associates
`LoaderFailure` with one outer EntryOutcome and never aborts the whole
load. Missing resolve identity is a plan error; a valid key returning
`None` is `UnresolvedKey`. Typed resolver errors and panics normalize
once to opaque `ResolverFailure`; `SpawnError` remains intact as
`LoaderFailure::Spawn`.

## Timer facade and operations

`TimerExt` is a sealed extension trait implemented only for Context:
`cordis-timer` cannot add inherent methods to a foreign core type. It
exposes three complete, synchronously fallible operations:

```rust
fn sleep(&self, delay: Duration)
    -> Result<Sleep, TimerRegistrationError>;

fn timeout<F: Future>(&self, delay: Duration, work: F)
    -> Result<Timeout<F>, TimerRegistrationError>;

fn interval(&self, period: Duration)
    -> Result<Interval, TimerRegistrationError>;
```

`Sleep`, `Timeout<F>`, and `Interval` are named opaque operation types
without `Clone`, `Copy`, `Debug`, equality, ordering, serde, or promised
`Unpin`. There is no public ArmedSleep, arm/defer/watch transport, raw
timer handle, callback, reset/cancel, Timer-as-Service, alias, throttle,
debounce, or Tokio representation. Core sees only a generic prepared
cleanup obligation.

Sleep and Timeout deadlines are monotonic and pinned at successful
construction. Work stays lazy and is owned only by `Timeout<F>`;
generation cleanup owns only timer cancellation and imposes no universal
`Send` or `'static` bound on `F` or its output. Each one-shot has exactly one
terminal result; polling `Sleep` or `Timeout<F>` again after that result is a
caller error and panics. `Interval` differs deliberately as a stream: after its
terminal cancellation item, every later poll returns `None`.

```rust
impl Future for Sleep {
    type Output = Result<(), TimerCancelled>;
}

#[derive(Debug)]
pub enum TimeoutOutcome<T> {
    Completed(T),
    Elapsed,
}

impl<F: Future> Future for Timeout<F> {
    type Output = Result<TimeoutOutcome<F::Output>, TimerCancelled>;
}
```

Timeout owns the hard work/deadline race: `Completed` can commit only
while the pinned deadline remains unelapsed after `F` reports Ready; at
an uncommitted boundary `Elapsed` wins. Deadline expiry is normal;
generation cancellation is a distinct error. Exactly one terminal
outcome commits. Dropping abandons the operation and `F` and disarms
cleanup when possible; generation cleanup never polls, moves, or drops
`F`. `Timeout<F>` adds no panic boundary around caller-owned work: a panic
while polling `F` follows ordinary Rust unwinding through the caller's poll.

`Interval: Stream<Item = Result<(), TimerCancelled>>` anchors its phase
at successful construction, first tick at anchor + period. A late poll
yields at most one overdue tick, drops older misses, and advances to the
first original-phase deadline strictly after now. Cadence never shifts
to delivery time and never bursts catch-up ticks. Generation
cancellation wins any uncommitted tick, emits exactly one `Err`, and
then all polls return `None`. Drop emits nothing; there is no natural
completion.

All constructors use one error:

```rust
#[non_exhaustive]
pub enum TimerRegistrationError {
    InactiveContext,
    TimerUnavailable,
    ZeroPeriod,
    DeadlineOutOfRange,
}
```

Validation precedence is local arguments, Context admission, Timer
environment/deadline preparation, cleanup-registration commit, then
operation delivery. `ZeroPeriod` applies only to interval; a zero
one-shot delay is valid. Every failure is atomic and returns no
born-terminal operation. Postconstruction generation termination is
`TimerCancelled`, never registration failure.

## Operation-specific errors and panic boundaries

There is no `CordisError`, no `CordisErrorCode`, no public defaulted
Result alias, no `code`/`code_with`/`error_code`/`inactive` helper, no
global code, and no blanket re-collapse conversion. `BoxError` remains
at the core crate root as an optional application convenience only.

The complete operation families are:

```text
RealmMappingError
ServiceLookupError  ServicePublishError  ServiceControlError
ConfigResolutionError<E>
EffectRegistrationError  EffectFailure
TaskRegistrationError
ListenerRegistrationError  DispatchError
SpawnError  ReadyError  RestartError  WaitStateError
UpdateError  EraSwapError  EraSwapFailure
PlanError  LoaderFailure  ResolverFailure
TimerRegistrationError  TimerCancelled
```

The matchable core resource families are:

```rust
#[non_exhaustive]
pub enum RealmMappingError {
    DuplicateService { service: String },
    ForeignRealm { service: String },
}

#[non_exhaustive]
pub enum ServiceLookupError {
    Unavailable { service: &'static str },
    ContractMismatch { service: &'static str },
}

#[non_exhaustive]
pub enum ServicePublishError {
    InactiveContext,
    DuplicatePublication { service: &'static str },
    ContractMismatch { service: &'static str },
}

#[non_exhaustive]
pub enum ServiceControlError {
    StalePublication { service: &'static str },
    MutationClosed { service: &'static str },
}

#[non_exhaustive]
pub enum ConfigResolutionError<E: std::error::Error> {
    ContractMismatch { service: &'static str },
    Compose(E),
}

#[non_exhaustive]
pub enum EffectRegistrationError {
    InactiveContext,
}

#[non_exhaustive]
pub enum TaskRegistrationError {
    InactiveContext,
    ExecutorUnavailable,
}
```

The lifecycle family is commit-aware:

```rust
pub enum PluginFailureKind {
    ReturnedError,
    Panic,
}

pub enum LifecycleOperation {
    Ready,
    WaitState,
    Restart,
    Update,
    EraSwap,
    Dispose,
    RemovePlugins,
}

#[non_exhaustive]
pub enum SpawnError {
    InactiveContext,
    InitialApply(PluginFailure),
    Interrupted,
}

#[non_exhaustive]
pub enum ReadyError {
    Recursion(LifecycleRecursion),
    Apply(PluginFailure),
}

#[non_exhaustive]
pub enum RestartError {
    Closed,
    Recursion(LifecycleRecursion),
    Apply(PluginFailure),
}

#[non_exhaustive]
pub enum WaitStateError {
    Elapsed,
    Recursion(LifecycleRecursion),
}

#[non_exhaustive]
pub enum UpdateError {
    PluginContractMismatch,
    Closed,
    Recursion(LifecycleRecursion),
    Control(InvocationFailure),
    AdmissionLost,
    Apply(PluginFailure),
}

#[non_exhaustive]
pub enum EraSwapFailure {
    SuccessorApply(PluginFailure),
    SuccessorLost,
}

#[non_exhaustive]
pub enum EraSwapError {
    PluginContractMismatch,
    Closed,
    Recursion(LifecycleRecursion),
    Incomplete(EraSwapFailure),
}
```

The Loader phase split is:

```rust
#[non_exhaustive]
pub enum PlanError {
    ForeignParent { parent: EntryId },
    MissingResolveIdentity { entry: EntryId },
    DuplicateInjectService { entry: EntryId, service: String },
    DuplicateIsolateService { entry: EntryId, service: String },
}

#[non_exhaustive]
pub enum LoaderFailure {
    UnresolvedKey { key: String },
    Resolver(ResolverFailure),
    Spawn(SpawnError),
}

pub enum ResolverFailureKind {
    ReturnedError,
    Panic,
}
```

`PluginFailure`, `EffectFailure`, `ResolverFailure`, and
`InvocationFailure` are opaque normalized values. Each exposes its
semantic kind and diagnostic text through read-only accessors — for
`EffectFailure` an `EffectFailureKind` distinguishing a returned error
from a contained panic. Plugin failure has no registration identity;
Resolver and Effect failures have no occurrence identity; Invocation
failure additionally exposes an optional `ListenerRegistrationId`.
`LifecycleRecursion` exposes only its `LifecycleOperation` and a public
correlation-only `FiberId`. Opaque failures have no public constructors,
original-error downcasts, or `Any` access.

Failure phases stay distinct. Service errors distinguish realm
derivation, lookup, publication, and exact control. Effect registration
and execution are separate. `Context::run` uses
`TaskRegistrationError::{InactiveContext, ExecutorUnavailable}`.
Lifecycle errors state their commit phase: spawn delivers no FiberHandle and
leaves no attempted resident; ready, restart, and update Apply failures
park `Failed` according to whether their commit occurred; era-swap
`Incomplete` is postclaim. `SpawnError::Interrupted` and era
`SuccessorLost` mean concurrent framework invalidation before handoff,
never caller cancellation. Dispose and typed group removal return only
`LifecycleRecursion`; cleanup failures remain secondary diagnostics.

Typed synchronous errors remain concrete where the operation can return
them, including `ConfigResolutionError<E>::Compose(E)`. Callback,
cleanup, resolver, and apply failures stay typed until the final
boundary that knows their type, then normalize exactly once to owned
kind, diagnostic text, and any justified correlation. The original
object, `Any`, downcast, and type identity are absent. Panic handling
follows the same boundary ownership. Direct calls to `Plugin::prepare`,
`Plugin::name`, `Plugin::inject`, `ConfigurableService::prepare_config`,
`ConfigurableService::compose_config`, and `PreparedPlugin::from_input`
retain ordinary Rust panic behavior; no universal Panic variants exist.

## Completeness

This inventory is exhaustive and needs no second declaration source.
Every module path, crate-root re-export, Context method, trait member
and bound, public field and variant, control capability, snapshot and
record, dispatch operation, observation item, Logger item, lifecycle
operation, error type, Loader item, and Timer export of the interface is
declared exactly once above at its canonical path or named as absent.
Every retained or changed group exists for a demonstrated consumer;
every removed group failed the deletion test or leaked representation;
every added group is required to express a frozen v3 rule. No
compatibility alias survives, and no semantic frontier remains.
