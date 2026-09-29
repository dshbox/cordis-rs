# Public API naming review

Non-normative review of the supported v3 inventory against production baseline
`4824196bbff6dada178ed230c344e12ad164919e`, fetched on 2026-09-29, under
[spec #200](https://github.com/dshbox/cordis-rs/issues/200).
The [public inventory](v3-public-interface.md) owns exact declarations and paths;
this review lists names to record their semantic assessment, not to define another API.

Conclusions are **Retain**, **Clarify** (retain the spelling and clarify usage),
or **Propose rename** (an independent reviewed change). Grouped rows
explicitly list members sharing a rationale. Standard Rust trait methods such as
Clone/Debug keep their standard meanings; their supported bounds remain owned by
the public inventory and compile contracts.

The [glossary](../CONTEXT.md) and accepted ADRs constrain the assessment.
[ADR 0037](adr/0037-public-interfaces-expose-semantics-not-representation.md)
requires one canonical spelling without aliases.
[ADR 0038](adr/0038-plugin-input-names-role-prepared-wrappers-name-stage.md) and
[0039](adr/0039-consumer-fiber-control-is-a-fiber-handle.md) settle Input and
FiberHandle. A new rename needs concrete misleading risk, an alternative,
source/package/authority/UI/example migration impact, and its own ticket before
implementation. Compatibility aliases do not bridge a pure rename.

## Preparation, creation and facades

Delivery: [#201](https://github.com/dshbox/cordis-rs/issues/201).
[Evidence](api-freeze-evidence.md#prepare-seal-and-spawn) and
[usage](consumer-guide.md#1-prepare-input-seal-it-then-spawn-and-retain-the-handle).

| Names / members | Conclusion | Semantic assessment |
| --- | --- | --- |
| `Context::new`; `Context` | Clarify | Creates one root Context into a new Cordis Runtime; Context is a view, not an independent owner or a general hierarchy |
| `Plugin`; `Config`, `Input`, `PrepareError`, `ApplyError`; `prepare` | Retain | Source configuration, complete runtime input and each operation's typed failure are distinct. Input names role, preparation names adaptation; Arc<P> delegates the same contract for P: Plugin + Sync |
| `Plugin::apply` | Clarify | Applies one typed input without promising origin-runtime affinity. Apply may run on a Cordis-owned runtime; current placement is ADR 0029 posture, not a frozen promise. Keep polls non-blocking; a rename would not communicate these scheduling boundaries |
| `Plugin::name`, `inject` | Clarify | Name is diagnostic, never lookup identity; inject declares lifecycle requirements. Sealing materializes both before lifecycle; it does not call them repeatedly during settlement |
| `PreparedPlugin`; `from_input` | Clarify | The wrapper names prepared stage and seals type association, not running state or the identity of the object that prepared the value |
| `PreparedPlugin::with_inject_overlay` | Clarify | Completes a dependency-declaration overlay before spawn; no Service-realm selection or dynamic lifecycle mutation |
| `Context::spawn`; `FiberHandle`, `FiberId` | Clarify | Spawn hands off a live quiescent Fiber; FiberHandle grants lifecycle control and has inert Drop, FiberId grants correlation only. ADR 0039 already resolves the misleading Fork spelling |
| `SpawnError`; `InactiveContext`, `InitialApply`, `Interrupted` | Retain | Names pre-allocation refusal, contained initial apply failure, and framework invalidation before handoff; Interrupted does not mean caller cancellation |
| `PluginFailure`, `PluginFailureKind`; `kind`, `diagnostic`; `ReturnedError`, `Panic` | Retain | Opaque apply failure retains normalized semantic cause/text, not heterogeneous error/downcast authority |
| `BoxError` | Clarify | Optional outer application erasure; use concrete Plugin associated error types until their owning normalization boundary |
| Core semantic modules: `plugin`, `lifecycle`, `service`, `event`, `effect`, `logger`, `observation` | Retain | Curated semantic families, independent of internal source layout; module event is singular |
| Core root whitelist: `Context`, `Plugin`, `PreparedPlugin`, `PreparedChange`, `InjectSpec`, `FiberHandle`, `FiberId`, `FiberState`, `UpdateOutcome`, `Service`, `ConfigurableService`, `ServiceRealm`, `Event`, `Scope`, `Routing`, `QueryOutcome`, `Logger`, `Level`, `BoxError`; matching cordis facade paths | Retain | The explicitly supported root conveniences and seven application modules mirror core contracts. Specialist paths remain those in the inventory; the facade adds no independent meaning or leaf re-export |

`cordis_core::__internal` is not a supported public naming proposal, even when
feature unification makes it reachable. Its published-sibling obligation is
reviewed under the [compatibility policy](compatibility-policy.md), separately
from downstream names. No new public rename is justified by this path.

## Context axes and exact Services

Delivery: [#202](https://github.com/dshbox/cordis-rs/issues/202).
[Evidence](api-freeze-evidence.md#context-axes-and-exact-services) and
[usage](consumer-guide.md#2-choose-service-placement-event-reachability-and-configuration-independently).

| Names / members | Conclusion | Semantic assessment |
| --- | --- | --- |
| `Context::root` | Clarify | Resets current Fiber, isolate, Scope and intercept in the same Runtime; no new Runtime or shutdown authority |
| `ServiceRealm`; `Context::new_service_realm`, `with_service_realms`, `with_isolated_service` | Retain | Opaque Runtime-local exact placement; private derivations never rendezvous and mappings have no fallback |
| `RealmMappingError`; `DuplicateService { service }`, `ForeignRealm { service }` | Retain | Ordered atomic mapping refusals, with diagnostic Service correlation |
| `Scope`; `Context::scope`, `with_child_scope` | Clarify | Event reachability position and derivation only; ancestry is not exposed and implies no ownership, Service or authorization meaning |
| `InjectSpec`; `none`, `require`, `require_configured`; `Default` | Clarify | Empty declaration and consuming prerequisite upserts; later require clears a configured layer, declaration order is nonsemantic |
| `Service`; `NAME` | Retain | Named typed value contract; NAME plus selected realm chooses an exact slot, not diagnostic Plugin identity |
| `ConfigurableService`; `Config`, `Layer`, `Resolved`, `PrepareError`, `ComposeError`; `prepare_config`, `compose_config` | Clarify | Source adaptation, retained prepared layer and final resolution; Service owns composition meaning and operation-specific errors |
| `Context::with_intercept`, `resolve_config` | Clarify | Prepared Service configuration overlay and typed resolution; intercept is not Event middleware or an authorization boundary |
| `ConfigResolutionError`; `ContractMismatch { service }`, `Compose` | Retain | Wrong named Service contract versus the Service's typed composition failure |
| `Context::provide`, `try_service` | Clarify | Admission of one occurrence versus exact visible lookup; lookup requires no declared injection membership |
| `ServicePublication`; `set`, `remove` | Clarify | Exact occurrence capability; set preserves target, consuming remove competes with cleanup, Drop is inert |
| `ServiceLookupError`; `Unavailable { service }`, `ContractMismatch { service }` | Retain | Missing/invisible selected occurrence versus wrong typed contract, without fallback |
| `ServicePublishError`; `InactiveContext`, `DuplicatePublication { service }`, `ContractMismatch { service }` | Retain | Generation admission refusal, occupied slot and incompatible typed contract remain distinct |
| `ServiceControlError`; `StalePublication { service }`, `MutationClosed { service }` | Clarify | Exact identity is checked first; a replaced occurrence is stale even when the former generation is closed |

No new spelling or domain decision is needed. These explanations retain the
glossary's independent axes and exact-publication authority.



## Lifecycle control and era replacement

Delivery: [#203](https://github.com/dshbox/cordis-rs/issues/203).
[Evidence](api-freeze-evidence.md#lifecycle-control-and-era-replacement) and
[usage](consumer-guide.md#4-choose-same-fiber-control-or-replace-the-era-then-retain-the-right-handle).

| Names / members | Conclusion | Semantic assessment |
| --- | --- | --- |
| `FiberState`; `Loading`, `Active`, `Pending`, `Unloading`, `Failed`, `Disposed` | Retain | Lifecycle publications, with no numeric order or default; Failed follows completed rollback |
| `FiberHandle::name`, `state`, `id`, `pending_missing` | Clarify | Diagnostic name, observed state, opaque correlation identity and normalized missing prerequisite names; none grants registry lookup |
| `FiberHandle::ready`, `wait_state` | Clarify | Current-target quiescence driver versus passive publication wait; ready can return Pending or current apply failure |
| `FiberHandle::restart`, `update`, `era_swap` | Clarify | Retry input, replace same-Fiber input, or end old identity before a fresh successor; no dead-handle respawn |
| `PreparedChange`; `from_input` | Clarify | Typed move-only one-attempt candidate, with no Plugin behavior or control; every attempted outcome consumes it |
| `UpdateOutcome`; `Committed`, `Vetoed` | Clarify | Committed input can settle Pending; Vetoed is normal precommit policy and does not return a reusable candidate |
| `Context::on_update`; `UpdateListener`; `UpdateNext::call` | Clarify | Sealed methodless policy subscription and consuming remaining-chain capability; framework invokes it only before update commit, not for era swap or postcommit recovery |
| `ReadyError`; `Recursion`, `Apply` | Retain | Exact self-wait refusal or normalized current-target apply failure |
| `WaitStateError`; `Elapsed`, `Recursion`, `DeadlineUnavailable` | Retain | Passive wait timeout, self-wait refusal or refusal to start the shared deadline thread. DeadlineUnavailable accurately names unavailable deadline infrastructure rather than falsely reporting elapsed time; added by #218 on the non-exhaustive enum, with no rename or alias migration |
| `RestartError`; `Closed`, `Recursion`, `Apply` | Retain | Admission/liveness refusal, self-wait refusal or committed retry failure |
| `UpdateError`; `PluginContractMismatch`, `Closed`, `Recursion`, `Control`, `AdmissionLost`, `Apply` | Clarify | Precommit type/liveness/recursion/control/revalidation failures versus postcommit Apply, which retains the new input |
| `EraSwapError`; `PluginContractMismatch`, `Closed`, `Recursion`, `Incomplete` | Clarify | Three preclaim refusals versus a committed replacement that cannot deliver a successor after final cleanup/convergence |
| `EraSwapFailure`; `SuccessorApply`, `SuccessorLost` | Retain | Successor-specific contained failure versus framework invalidation before handoff |
| `LifecycleRecursion`; `operation`, `fiber_id`; `LifecycleOperation::{Ready, WaitState, Restart, Update, EraSwap, Dispose, RemovePlugins}` | Retain | Typed operation and exact allocation correlation for recursion refusal; no erased name-based self-wait heuristic |

Accepted Input/FiberHandle spelling remains unchanged. Explanation of commit
phase and identity continuity addresses the ambiguity without a rename ticket.

## Generation resources and explicit teardown

Delivery: [#204](https://github.com/dshbox/cordis-rs/issues/204).
[Evidence](api-freeze-evidence.md#generation-ownership-and-consumer-teardown) and
[usage](consumer-guide.md#5-register-resources-with-their-generation-and-separate-ownership-from-attribution).

| Names / members | Conclusion | Semantic assessment |
| --- | --- | --- |
| `Context::effect`, `effect_sync` | Clarify | Register async/synchronous at-most-once generation cleanup; sync means short immediate work, not infallible work |
| `CleanupResult`; `into_outcome` | Retain | Sealed adaptation of unit or typed Result into normalized cleanup outcome; not an extensible transport protocol |
| `EffectRegistration`; `dispose`, `disarm` | Clarify | Exact consuming cleanup claim or suppression; inert Drop, winning dispose independently completes, false means another claim won |
| `EffectFailure`, `EffectFailureKind`; `kind`, `diagnostic`; `ReturnedError`, `Panic` | Retain | Normalized cleanup cause and owned diagnostic; no original error type/downcast authority |
| `EffectRegistrationError`; `InactiveContext` | Retain | Current generation refuses retained cleanup admission |
| `Context::run` | Clarify | Registers generation-owned task abort/join cleanup; no returned user task handle |
| `Context::spawn_attributed` | Clarify | User-owned Tokio task with transferred live recursion attribution; does not register generation cleanup |
| `TaskRegistrationError`; `InactiveContext`, `ExecutorUnavailable` | Retain | Generation refusal versus no available executor; failure starts no task |
| `FiberHandle::dispose` | Clarify | Awaits full terminal barrier for this Fiber; does not end spawn descendants or root registrations |
| `Context::remove_plugins` | Clarify | Typed current-allocation removal, not diagnostic-name lookup, parent cascade or application shutdown |

The names retain their established ownership and exact-claim meanings. Consumer
ordering does not warrant a core shutdown/tree API or a new public rename.

## Event roles, dispatch and exact claims

Delivery: [#205](https://github.com/dshbox/cordis-rs/issues/205).
[Evidence](api-freeze-evidence.md#event-dispatch-and-exact-claims) and
[usage](consumer-guide.md#7-select-an-event-role-and-routing-then-await-the-intended-completion).

| Names / members | Conclusion | Semantic assessment |
| --- | --- | --- |
| `Event`; `NAME`, `Args`, `Output` | Retain | Named compatible typed contract; marker identity does not route or impose universal Clone/Sync bounds |
| `Routing`; `Unscoped`, `Scoped` | Clarify | Explicit eligibility selection in one Runtime; Scoped(root) is not Unscoped |
| `Listener`; `observer`, `observer_sync`, `responder`, `responder_sync`, `mapper`, `mapper_sync`, `around` | Clarify | Sealed methodless capability with explicit semantic roles; sync denotes immediate completion, not infallibility; no around_sync contract |
| `StatefulCallback`; `with_state` | Clarify | Opaque adapter composition creates invocation-local state exactly once after claim, not shared listener state |
| `ListenerOptions`; `prepend`, `global`, `once`, `is_prepend`, `is_global`, `is_once`; `Default` | Clarify | Consuming selection/order/claim policy and read-only facts; default append/scoped/repeatable, once means claim rather than success |
| `Context::on`, `on_with`; `ListenerRegistration::remove` | Clarify | Register/control one exact occurrence; consuming remove and inert Drop do not revoke claimed invocation |
| `ListenerRegistrationId` | Retain | Opaque correlation identity, with no lookup/removal authority |
| `Next`; `call` | Clarify | Consuming remaining waterfall chain once, not arbitrary redispatch |
| `Context::emit`, `emit_parallel`, `query`, `waterfall`, `waterfall_query` | Clarify | Four completion-aware primitives and one derived query tail; no detached completion after pending-future cancellation |
| `QueryOutcome`; `Miss`, `Answer` | Retain | Explicit absence/presence; false, zero and empty values are answers |
| `ListenerRole`; `Observer`, `Responder`, `Mapper`, `Around` | Retain | Callback semantic roles, shared by error/observation correlation |
| `EventOperation`; `Emit`, `EmitParallel`, `Query`, `Waterfall` | Retain | Exactly four primitives; waterfall_query narrates its derived operations |
| `DispatchOutcomeKind`; `Completed`, `Answered`, `Missed`, `Failed` | Clarify | Completed-operation narration, not guaranteed subscriber receipt or an audit journal |
| `InvocationFailure`, `InvocationFailureKind`; `registration_id`, `kind`, `diagnostic`; `ReturnedError`, `Panic` | Retain | Normalized invocation cause/text and optional exact listener correlation; None denotes framework tail |
| `ParallelFailures`; `failures` | Retain | Nonempty read-only failures in effective listener order, including the one-failure case |
| `ListenerRegistrationError`; `InactiveContext`, `EventContractMismatch { event }` | Retain | Generation admission refusal versus incompatible named Event contract |
| `DispatchError`; `EventContractMismatch { event }`, `ForeignScope`, `IncompatibleRole { operation, role }`, `Invocation`, `Parallel` | Retain | Preflight contract/routing/role failures versus correlated invocation completion failures |

The inventory now includes existing canonical `event::with_state`; its public
export and body contract already existed. No new operation, alias or rename is added.

## Runtime observations, snapshots and Logger

Delivery: [#206](https://github.com/dshbox/cordis-rs/issues/206).
[Evidence](api-freeze-evidence.md#runtime-observations-and-logger) and
[usage](consumer-guide.md#8-observe-committed-facts-and-install-explicit-logger-exporters).

| Names / members | Conclusion | Semantic assessment |
| --- | --- | --- |
| `Context::observe_runtime`; `RuntimeObserver` | Clarify | Sealed Observer capability for detached postcommit subscription; not an Event, replay channel or source-operation completion dependency |
| `Context::runtime_snapshot`; `RuntimeSnapshot::fibers`, `services` | Clarify | Flat current records, without globally atomic cross-collection or semantic ordering promises |
| `FiberSnapshot`; `id`, `role`, `name`, `state`, `missing_services` | Retain | Read-only correlation, root/ordinary role, diagnostic name, lifecycle publication and missing prerequisite projection |
| `ServiceSnapshot`; `id`, `service`, `realm`, `provider`, `visible` | Clarify | Read-only current occupied occurrence, exact slot/provider correlation and visibility; occupied Loading is not visible |
| `FiberRole`; `Root`, `Ordinary` | Retain | Permanent root versus ordinary Fiber; no representation or tree topology implied |
| `ServicePublicationId`, `ScopeId` | Retain | Opaque Runtime-local correlation only, with no occurrence mutation or Scope routing capability |
| `ObservationRouting`; `Unscoped`, `Scoped` | Clarify | Reports eligibility correlation; its ScopeId does not grant a Scope |
| `ResidencyChange::{Admitted, Removed}`; `ListenerChange::{Registered, Unregistered}` | Retain | Postcommit exact residency/listener occurrence facts |
| `RuntimeObservation::FiberResidency { change, fiber }`, `FiberState { fiber, previous, current }` | Retain | Admitted/removed residency and state transitions, distinct from handle ownership |
| `RuntimeObservation::ServiceVisibility { service, realm, previous, current }` | Retain | Exact slot visibility occurrence transition, not same-occurrence payload mutation |
| `RuntimeObservation::ListenerRegistration { change, listener, event, role, scope, options }` | Retain | Exact listener correlation and semantic registration metadata, not storage/claim controls |
| `RuntimeObservation::DispatchCompleted { operation, event, routing, outcome }` | Clarify | Completed primitive narration; best-effort delivery does not establish audit or callback receipt order |
| `Context::logger`; `Logger::with_name`, `name`, `log`, `debug`, `info`, `warn`, `error` | Clarify | Named Runtime foundation channel; not a Service or durable journal |
| `Level`; associated constants `Debug`, `Info`, `Warn`, `Error`; `as_str` | Retain | Opaque semantic severity with low-to-high order and wire names, no numeric enum contract |
| `LogRecord`; `sequence`, `timestamp`, `channel`, `level`, `text` | Clarify | Immutable assignment sequence, wall-clock timestamp and semantic fields; sequence is not receipt order or persistent identity |
| `Exporter`; `export`, `min_level`, `default_level` | Clarify | Record callback and channel threshold/default; None means no override, not disable |
| `Context::add_exporter`; `ExporterRegistration::remove` | Clarify | Exact generation-owned occurrence with inert Drop; consuming removal does not revoke retained in-flight snapshots |
| `BufferExporter`; `new`, `snapshot`, `clear`; `BufferSizeZero` | Retain | Explicit bounded nonzero-capacity receipt-order adapter, not automatically installed |

The observation and Logger names remain accurate with these boundaries. No new
public spelling, observation Event or collection API is justified.

## Loader plan, adaptation and handoff

Delivery: [#207](https://github.com/dshbox/cordis-rs/issues/207).
[Evidence](api-freeze-evidence.md#loader-plan-execution-and-handoff) and
[usage](consumer-guide.md#9-freeze-a-loader-plan-inspect-every-outcome-and-retain-delivered-handles).

| Names / members | Conclusion | Semantic assessment |
| --- | --- | --- |
| `cordis_loader::{plan, resolver, outcome}`; root conveniences `LoadPlanBuilder`, `LoadPlan`, `EntryId`, `PluginEntry`, `EntryGroup`, `PluginResolver`, `LoadOutcome` | Retain | Canonical semantic leaf modules and curated root paths; application facade does not re-export the leaf |
| `LoadPlanBuilder`; `new`, `add_plugin`, `add_group`, `finish` | Clarify | Failure-atomic source admission followed by immutable freeze, not a live Fiber tree or patchable Runtime |
| `LoadPlan`; `load` | Clarify | Reusable frozen sequencing plan, executing partial outcomes with separate final handoff ownership |
| `EntryId` | Clarify | Opaque plan-lineage correlation, not numeric index, path, resolve key or Fiber control |
| `PluginEntry`; `key`, `name`, `config`, `disabled`, `inject`, `isolate` | Clarify | Mutable serialized source; key/name choose repeatable resolve metadata, config is required, disable prunes descendants, inject and placement are independent |
| `EntryGroup`; `name` | Clarify | Structural sequencing source syntax; name is not retained as frozen-plan/outcome identity |
| `InjectEntry`; `Required`, `Configured { service, config }` | Retain | Source dependency declarations, with optional target-specific typed configuration preparation |
| `IsolateEntry`; `service`, `policy`; `RealmPolicy::{Private, Shared { label }}` | Clarify | Execution-local source placement policy; labels never become core identity or cross-execution rendezvous |
| `PlanError`; `ForeignParent { parent }`, `MissingResolveIdentity { entry }`, `DuplicateInjectService { entry, service }`, `DuplicateIsolateService { entry, service }` | Retain | Failure-atomic parent/identity/axis validation with exact rejected-source correlation |
| `PluginResolver`; `Error`, `resolve` | Clarify | Externally implementable synchronous typed adaptation before admission; closure blanket implementation has the same meaning |
| `PluginRequest`; `resolve_key`, `config`, `inject` | Clarify | Borrowed source adaptation inputs; no entry identity, Runtime, placement or topology authority |
| `prepare_plugin_json`, `prepare_service_json`; `JsonPrepareError::prepare_error` | Clarify | Deserialize then typed preparation; helper preserves concrete preparation failure, resolver alone normalizes later; Error::source does not expose arbitrary non-static preparation errors |
| `ResolverFailure`, `ResolverFailureKind`; `kind`, `diagnostic`; `ReturnedError`, `Panic` | Retain | Once-normalized opaque resolver cause/text, not original error/downcast identity |
| `LoaderFailure`; `UnresolvedKey { key }`, `Resolver`, `Spawn` | Retain | Unknown valid key, normalized adaptation failure or intact core creation failure for one outcome |
| `EntryOutcome`; `Group { id }`, `Disabled { id }`, `Pruned { id, disabled_ancestor }`, `Spawned { id, resolve_key, fiber_handle }`, `Failed { id, resolve_key, failure }`; `id` | Clarify | One complete ordered result per source entry; pruning is disable-driven, row failure does not abort descendants |
| `LoadOutcome`; `entries`, `entry`, `fiber_handles`, `is_ok` | Clarify | Delivered partial results and caller-owned controls; is_ok means no Failed entry, not universal Active; Drop is inert |

No tree, async resolver, key-index alias or public rename is needed. Published
sibling compatibility remains distinct from unsupported downstream __internal use.

## Timer operations and terminal ownership

Delivery: [#208](https://github.com/dshbox/cordis-rs/issues/208).
[Evidence](api-freeze-evidence.md#timer-construction-arbitration-and-terminal-ownership) and
[usage](consumer-guide.md#10-check-timer-registration-then-distinguish-elapsed-from-cancellation).

| Names / members | Conclusion | Semantic assessment |
| --- | --- | --- |
| `cordis_timer` root: `TimerExt`, `Sleep`, `Timeout`, `Interval`, `TimeoutOutcome`, `TimerCancelled`, `TimerRegistrationError` | Retain | Flat canonical leaf surface; shapes implementation module is private and the application facade adds no second path |
| `TimerExt`; `sleep`, `timeout`, `interval` | Clarify | Sealed Context extension constructs complete operations with synchronous registration refusal, not async registration or raw transports |
| `Sleep` | Clarify | Opaque one-shot Future with completion or generation cancellation; one terminal result and repoll panic |
| `Timeout` | Clarify | Caller-owned lazy work/deadline arbitration; cleanup owns cancellation only and adds no work panic boundary |
| `Interval` | Clarify | Named result Stream anchored at construction, no burst/phase shift; cancellation emits one error then fused None, Drop emits nothing |
| `TimeoutOutcome`; `Completed`, `Elapsed` | Retain | Caller work output versus normal deadline expiry; neither represents generation cancellation |
| `TimerCancelled` | Retain | Distinct terminal error after successful construction and generation termination |
| `TimerRegistrationError`; `InactiveContext`, `TimerUnavailable`, `ZeroPeriod`, `DeadlineOutOfRange` | Retain | Atomic pre-delivery generation/environment/argument/deadline refusals; TimerUnavailable includes missing driver |

Standard Future/Stream `poll`/`poll_next`, Output/Item and associated pinning
contracts remain owned by those traits and the Timer authority. No public reset,
cancel, raw handle, timer Service, alias or rename is justified.

## Aggregate naming disposition

The eight path sections cover the supported default-feature core inventory,
application facade and explicit Loader/Timer leaves. Integration checks all 80
canonical core specialist names against the normative table and all public method
spellings in the supported source families; field/variant assessments are grouped
explicitly above. Lexical presence is a completeness backstop, not semantic proof:
the per-path source/contract reviews supply the meaning and usage assessment.

**Propose rename: none.** No concrete misleading risk survives the established
glossary, accepted ADR constraints and the clarification above. There is therefore
no new public rename ticket or queued deliberate break for this candidate. Existing
Input/FiberHandle decisions are retained. A later justified proposal must become
its own ticket with risk, one canonical alternative, migration impact and real
dependencies; no public compatibility alias is the default migration path here.

[Spec #200](https://github.com/dshbox/cordis-rs/issues/200) and the
[freeze recommendation](api-freeze-recommendation.md) retain the final scope and
blocker disposition. This naming review introduces no declarations or new domain
meaning and requires no new glossary entry or ADR.
