# Public API naming review

Non-normative review of the supported v3 inventory against production baseline
`ed07d3149a5711541eaabd34633122e6eb3c52bd`, under
[spec #200](https://github.com/dshbox/cordis-rs/issues/200).
The [public inventory](v3-public-interface.md) owns exact declarations and paths;
this review lists names to record their semantic assessment, not to define another API.

Conclusions are **保留** (retain), **改善说明** (retain the spelling and clarify
usage), or **建议改名** (propose an independent reviewed rename). Grouped rows
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
| `Context::new`; `Context` | 改善说明 | Creates one root Context into a new Cordis Runtime; Context is a view, not an independent owner or a general hierarchy |
| `Plugin`; `Config`, `Input`, `PrepareError`, `ApplyError`; `prepare`, `apply` | 保留 | Source configuration, complete runtime input and each operation's typed failure are distinct. Input names role, preparation names adaptation; Arc<P> delegates the same contract for P: Plugin + Sync |
| `Plugin::name`, `inject` | 改善说明 | Name is diagnostic, never lookup identity; inject declares lifecycle requirements. Sealing materializes both before lifecycle; it does not call them repeatedly during settlement |
| `PreparedPlugin`; `from_input` | 改善说明 | The wrapper names prepared stage and seals type association, not running state or the identity of the object that prepared the value |
| `PreparedPlugin::with_inject_overlay` | 改善说明 | Completes a dependency-declaration overlay before spawn; no Service-realm selection or dynamic lifecycle mutation |
| `Context::spawn`; `FiberHandle`, `FiberId` | 改善说明 | Spawn hands off a live quiescent Fiber; FiberHandle grants lifecycle control and has inert Drop, FiberId grants correlation only. ADR 0039 already resolves the misleading Fork spelling |
| `SpawnError`; `InactiveContext`, `InitialApply`, `Interrupted` | 保留 | Names pre-allocation refusal, contained initial apply failure, and framework invalidation before handoff; Interrupted does not mean caller cancellation |
| `PluginFailure`, `PluginFailureKind`; `kind`, `diagnostic`; `ReturnedError`, `Panic` | 保留 | Opaque apply failure retains normalized semantic cause/text, not heterogeneous error/downcast authority |
| `BoxError` | 改善说明 | Optional outer application erasure; use concrete Plugin associated error types until their owning normalization boundary |
| Core semantic modules: `plugin`, `lifecycle`, `service`, `event`, `effect`, `logger`, `observation` | 保留 | Curated semantic families, independent of internal source layout; module event is singular |
| Core root whitelist: `Context`, `Plugin`, `PreparedPlugin`, `PreparedChange`, `InjectSpec`, `FiberHandle`, `FiberId`, `FiberState`, `UpdateOutcome`, `Service`, `ConfigurableService`, `ServiceRealm`, `Event`, `Scope`, `Routing`, `QueryOutcome`, `Logger`, `Level`, `BoxError`; matching cordis facade paths | 保留 | The explicitly supported root conveniences and seven application modules mirror core contracts. Specialist paths remain those in the inventory; the facade adds no independent meaning or leaf re-export |

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
| `Context::root` | 改善说明 | Resets current Fiber, isolate, Scope and intercept in the same Runtime; no new Runtime or shutdown authority |
| `ServiceRealm`; `Context::new_service_realm`, `with_service_realms`, `with_isolated_service` | 保留 | Opaque Runtime-local exact placement; private derivations never rendezvous and mappings have no fallback |
| `RealmMappingError`; `DuplicateService { service }`, `ForeignRealm { service }` | 保留 | Ordered atomic mapping refusals, with diagnostic Service correlation |
| `Scope`; `Context::scope`, `with_child_scope` | 改善说明 | Event reachability position and derivation only; ancestry is not exposed and implies no ownership, Service or authorization meaning |
| `InjectSpec`; `none`, `require`, `require_configured`; `Default` | 改善说明 | Empty declaration and consuming prerequisite upserts; later require clears a configured layer, declaration order is nonsemantic |
| `Service`; `NAME` | 保留 | Named typed value contract; NAME plus selected realm chooses an exact slot, not diagnostic Plugin identity |
| `ConfigurableService`; `Config`, `Layer`, `Resolved`, `PrepareError`, `ComposeError`; `prepare_config`, `compose_config` | 改善说明 | Source adaptation, retained prepared layer and final resolution; Service owns composition meaning and operation-specific errors |
| `Context::with_intercept`, `resolve_config` | 改善说明 | Prepared Service configuration overlay and typed resolution; intercept is not Event middleware or an authorization boundary |
| `ConfigResolutionError`; `ContractMismatch { service }`, `Compose` | 保留 | Wrong named Service contract versus the Service's typed composition failure |
| `Context::provide`, `try_service` | 改善说明 | Admission of one occurrence versus exact visible lookup; lookup requires no declared injection membership |
| `ServicePublication`; `set`, `remove` | 改善说明 | Exact occurrence capability; set preserves target, consuming remove competes with cleanup, Drop is inert |
| `ServiceLookupError`; `Unavailable { service }`, `ContractMismatch { service }` | 保留 | Missing/invisible selected occurrence versus wrong typed contract, without fallback |
| `ServicePublishError`; `InactiveContext`, `DuplicatePublication { service }`, `ContractMismatch { service }` | 保留 | Generation admission refusal, occupied slot and incompatible typed contract remain distinct |
| `ServiceControlError`; `StalePublication { service }`, `MutationClosed { service }` | 改善说明 | Exact identity is checked first; a replaced occurrence is stale even when the former generation is closed |

No new spelling or domain decision is needed. These explanations retain the
glossary's independent axes and exact-publication authority.
