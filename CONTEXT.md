# Cordis

Cordis is a consumer-agnostic runtime foundation: one Runtime hosts Fibers
executing Plugin behavior, coordinates their Services and Events, and
publishes observation of the whole. This glossary is the canonical domain
and protocol vocabulary of the v3 architecture; every normative document
and every implementation uses these terms with exactly these meanings.

## Language

### Runtime and lifecycle language

**core**
The consumer-agnostic runtime foundation: Fiber lifecycle and settlement,
Services and dependency coordination, Event dispatch, generation cleanup,
Registry residency, and logging. Consumer-facing time operations and
declarative loading live in optional leaf crates over narrow core seams;
policy that serves one kind of consumer never enters core.
_Avoid_: framework

**Runtime**
One complete Cordis running world: shared root state, stores, the Registry,
identity sources, and one permanent root Fiber. A Runtime begins with its
root Context, and every Context derived from that root remains a view into
the same Runtime. Where prose could be ambiguous, say Cordis Runtime to
distinguish it from a Tokio runtime.
_Avoid_: Cordis instance

**Context**
A cheap immutable view into one Runtime, selecting a current Fiber together
with independent isolate, Scope, and intercept axes. Derivation changes
only the selected axis; a Context has no independent stable domain
identity, and its derivation lineage carries no meaning beyond those
explicit axes.
_Avoid_: runtime, container, Context hierarchy

**root context**
The distinguished Context backed by the Runtime's permanent root Fiber. It
is a Context into the same Runtime, not a separate owner or running world;
`Root` and root state are implementation vocabulary.
_Avoid_: Root, root runtime

**Plugin**
A reusable behavior or declaration object that Cordis applies by spawning a
Fiber. It is not the live identity of an application.
_Avoid_: plugin instance

**Plugin input**
The semantically complete typed `Plugin::Input` value that `Plugin::apply`
borrows. `Plugin::prepare(Config)` produces such an input before lifecycle
admission; `PreparedPlugin::from_input` seals Plugin behavior with one value
of its declared Input contract for creation, while `PreparedChange::from_input`
seals one complete replacement input for update or era replacement. The type
association does not encode which particular Plugin object produced a value.
_Avoid_: Prepared as the Plugin input role, raw config, erased config, live Plugin state

**Fiber**
The era-local live lifecycle identity for one concrete execution
allocation, owning that allocation's lifecycle state and cleanup
registrations. Restart and same-Fiber update preserve it; era swap ends it
and creates a fresh Fiber. A Runtime also owns one permanent root Fiber.
_Avoid_: plugin instance, FiberHandle

**FiberId**
An opaque Runtime-local correlation identity for one Fiber allocation. It
survives that Fiber's state changes and later observation, while an era
swap creates a fresh FiberId. It grants no lookup, residency, or lifecycle
authority and is not a persistent or cross-process identity.
_Avoid_: UID, Registry key, lifecycle handle

**FiberHandle**
A cloneable consumer control handle referring to one non-root Fiber. It grants
lifecycle operations without owning Fiber residency or lifetime; dropping every
FiberHandle does not end the Fiber.
_Avoid_: Fork, Fiber, FiberRef, FiberHandler, plugin instance

**residency**
The Runtime and Registry keep a Fiber strongly resident and addressable.
Residency does not imply lifecycle ownership, and dropping every FiberHandle
changes neither residency nor lifecycle.
_Avoid_: ownership, reachability through a FiberHandle

**spawn origin**
The Context — and therefore the originating Fiber and view — from which a
spawn was initiated. It records provenance and supplies the spawning
Context; it implies no parenthood or teardown responsibility and is
independent of Scope ancestry, Event ancestry, isolate inheritance, and
lifecycle ownership.
_Avoid_: parent, owner, ancestor

**lifecycle ownership**
Explicit responsibility for ending a lifecycle and enforcing teardown
ordering: within a Fiber, each apply generation owns the cleanup
obligations that commit while its gate is open. Ordinary spawning
establishes residency and a spawn origin but no Fiber-to-Fiber ownership,
so consumers compose delivered FiberHandles explicitly (including reverse
spawn-order Roster teardown) and a spawned Fiber may outlive its spawn
origin.
_Avoid_: residency, spawn origin, parent/child, `Lifetime`, `DisposalScope`

**era swap**
The identity-breaking replacement of a Fiber by a freshly spawned
successor: the old Fiber ends fully, and the successor replays the captured
creation recipe with the same Plugin behavior and newly supplied
Plugin input, receiving a fresh FiberId, fresh era-local dependency edges,
a fresh sibling-era Scope beneath the captured spawn-origin Scope, and
fresh publication occurrences. The successor inherits neither the source
Scope nor publication occurrences, and no identity crosses the replacement;
the identity-continuous sibling operation is update, which keeps the same
Fiber and its resolved edges.
_Avoid_: reload cycle (that is settle pass vocabulary), respawn

### Interaction language

**Service**
A Runtime-local semantic value contract identified by Service name. Within
one Runtime, one name denotes one compatible contract; slot placement and
provider-publication occurrences are distinct from Service identity.
_Avoid_: provider, service slot

**service realm**
A `ServiceRealm`: a Runtime-local placement identity used to form an exact
Service slot `(Service, service realm)`. A realm has no implied hierarchy,
fallback, textual-name rendezvous, Event routing, or lifecycle ownership.
_Avoid_: namespace, parent realm, realm label

**isolate**
The Context axis that resolves each Service to one exact service realm.
Isolation neither renames a Service nor introduces fallback, Service
inheritance, Event reachability, or Context ancestry.
_Avoid_: namespace, Service scope

**provider publication**
One successful publication occurrence by a Fiber generation into one exact
Service slot. Exact occurrence identity governs visibility, replacement,
stale-cleanup safety, mutation/removal, and dependency assignment; it is
not another Service identity.
_Avoid_: Service, provider Fiber

**SemanticTarget**
The semantic value a Fiber settles and converges against: committed
effective apply input plus every exact dependency slot, each assigned
`Missing` or one exact visible provider publication. Equality drives
settlement, failed-target parking, retry eligibility, durable drift, and
ready convergence; mechanism counters, pointers, index state, and
notification history are excluded.
_Avoid_: target epoch, dependency fingerprint, notification version

**intercept**
The immutable Context axis for ordered Service-specific configuration
composition. Matching layers compose outer-to-inner under the relevant
Service contract; intercept is not Event middleware, authorization,
routing, or Service placement.
_Avoid_: Event interceptor, Service realm

**Event**
A Runtime-local named typed semantic contract over its invocation and
result behavior. Within one Runtime, one Event name binds one compatible
contract; listener registrations are independent occurrences, not Event
identity.
_Avoid_: listener registration, Rust type identity

**Scope**
A Runtime-local rooted Event-reachability relation. Scoped routing reaches
a listener exactly when its registration Scope is an ancestor-or-self of
the dispatch Scope, unless the listener is explicitly global; Scope implies
no lifecycle, Service, Fiber-parenthood, authorization, or Context
relation.
_Avoid_: Context hierarchy, lifecycle parent, isolate

**Runtime observation**
A read-only semantic projection or postcommit record describing
authoritative Runtime facts — exactly Fiber residency, Fiber state, Service
visibility, listener registration, and primitive dispatch completion
records, plus the complementary flat snapshot. Observation exposes no
Registry topology or operational capability, and its delivery never drives
protocol truth.
_Avoid_: internal Event, audit log, Registry snapshot

### Protocol language

**settle pass**
One of the three named compositions of the settle protocol — initial,
convergence, and restart — each driving a Fiber toward its current
SemanticTarget and out again through the release-time drift recheck. A
pass's ordering is protocol vocabulary owned by core, never knowledge a
caller re-derives.
_Avoid_: settle loop, reload cycle, settle choreography

**settle context**
Code running inside a Fiber's own settle pass or teardown: apply bodies,
disposers, and the Fiber's generation-owned tasks during a drain. The
Fiber's lifecycle operations refuse a synchronous self-wait from its settle
context with typed `LifecycleRecursion` instead of deadlocking. Raw
`tokio::spawn` starts outside that task-local context; user subtasks that
remain in the settle dependency graph carry it explicitly through
`Context::spawn_attributed`, and inherited frames expire when their source
settle scope ends. Detached Runtime-observation delivery is outside the source
Fiber's settle context.
_Avoid_: re-entrancy guard, settle callback

### Loader and consumer language

**LoadPlan**
One immutable, validated declarative Loader capability. Its structural
parentage and groups determine execution sequencing only — never Fiber
parenthood, Service realm, Scope ancestry, Registry grouping, or lifecycle
ownership. Repeated execution preserves entry correlation while producing
fresh lifecycle outcomes.
_Avoid_: deployment owner, Fiber hierarchy, entry tree

**EntryId**
An opaque correlation identity for one declarative entry in one LoadPlan
lineage. Cloning or sharing a plan preserves its EntryIds; independently
rebuilding equivalent content creates fresh identities. It is neither a
tree address nor a Fiber, Plugin, Registry, persistence, or path identity.
_Avoid_: entry index, path id, FiberId

**resolve key**
The string a Loader Plugin entry resolves its Plugin by — the entry's
`key`, falling back to its `name`. Load outcomes report it as execution
metadata; `EntryId` remains the entry correlation identity and resolve keys
may repeat.
_Avoid_: EntryId, Plugin identity, FiberId

**Roster**
The spawn-ordered accumulation of delivered FiberHandles a consumer holds across
boot and teardown: reported in spawn order, disposed in reverse spawn
order, attempt-all. Roster composition is consumer vocabulary; its
reference home is the shared Harness helpers.
_Avoid_: registry (that is core's Registry), fleet

**Harness**
The consumer-side composition layer that retains delivered FiberHandles and owns
explicit boot and teardown policy — concretely the shared example helpers
whose Roster records FiberHandles in spawn order and disposes them in reverse.
Harness composition is consumer policy, never a core ownership relation.
_Avoid_: core lifecycle owner, Runtime shutdown

**consumer**
Code that exercises the public interface of the Cordis crates for its own
ends — an example, an application, or a Harness. The runnable examples are
the consumers of record for interface decisions.
_Avoid_: user, client

**seam**
The location of a module's interface. An external seam exposes complete
semantic operations or immutable projections while consumer policy remains
on the consumer's side. An internal seam joins hidden protocol owners
through the narrow complete capability one owner requires from another. A
seam is neither source-file layout nor partial protocol machinery.
_Avoid_: hook, plugin point, public storage view

### Registry-local architecture vocabulary

These two terms are Registry-local architecture vocabulary. They describe
Registry internals and are not part of the primary domain model or external
Runtime observation.

**PluginKey**
An opaque Registry input identifying plugin grouping equivalence: typed
Plugins group by Rust type identity, and erased Plugins receive an
anonymous key. A PluginKey identifies neither a particular PluginGroup
allocation nor a resident occurrence, spawn lineage, or lifecycle owner.
_Avoid_: plugin instance id, group-allocation id, seat id

**PluginGroup**
The current Registry-private residency allocation selected by a PluginKey,
containing resident Fibers sharing that grouping relation and only metadata
invariant for the relation. The allocation may be pruned and recreated; it
is neither an interface, a lifecycle owner, nor a stable plugin-instance
identity.
_Avoid_: plugin seat, registry runtime

### Language exclusions

No canonical v3 concept exists for a plugin instance, a stable plugin seat,
a general Context hierarchy, a Service scope, a Registry runtime, a UID, or
an ownership-by-residency synonym; those spellings appear only as avoided
terms. There is likewise no established `Lifetime` or `DisposalScope`
abstraction — lifecycle identity and cleanup registrations belong to Fibers
and their apply generations.
