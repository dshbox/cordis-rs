# Cordis v3 architecture

This document is the sole normative entry point to the approved Cordis
v3 architecture. It states the settled target in current tense — how the
system is composed, which invariants hold, and how the parts interact —
and it is complete without any other version's documents. There is no
separate v3 index: reading starts here.

## Status, scope, and reading map

This document describes the approved v3 target even where production
Rust still differs from it. The normative set below — not current
code — is the contract an implementation or a review is measured
against, and no document in it delegates a v3 rule to current or
historical material.

| Document | Owns | Status |
| --- | --- | --- |
| [CONTEXT.md](../CONTEXT.md) | Canonical domain and protocol vocabulary | Normative glossary |
| This document | Present-tense target composition, invariants, internal interactions, concurrency, and seams | Normative architecture |
| [docs/v3-public-interface.md](v3-public-interface.md) | The exhaustive approved public surface and every caller-visible contract | Normative interface inventory |
| ADRs 0028–0038 (see the [decision index](#decision-index)) | The eleven independent hard-to-reverse rules and their complete rationale | Normative decisions |
| [docs/v3-upstream-parity-ledger.md](v3-upstream-parity-ledger.md) | The relationship of every pinned-upstream fact to its v3 disposition, with direct evidence | Normative parity record |

[docs/v3-migration.md](v3-migration.md) is **optional and
non-normative**: it records compatibility impact, historical
provenance, and test evidence for moving to this baseline. No v3 rule
lives there, and deleting it leaves the required-reading closure above
complete.

Every atomic conclusion has exactly one normative home: definitions in
the glossary, composition and invariants in this document,
hard-to-reverse choices and their rationale in the ADRs, exact
declarations and caller-visible contracts in the interface inventory,
and upstream relationships in the parity ledger. Where a rule meets the
decision threshold, this document states its architectural consequence
and links the owning ADR rather than re-deriving its alternatives; the
ADR's own self-contained statement of its rule is the only intentional
restatement. Exact public spellings, signatures, bounds, and result
shapes never appear here; the interface inventory owns them.

The only external architectural authority is the Cordis TypeScript
source pinned at commit
`8cc9e33fab69e2d0476d126baaf2acb24e6a6ab4`. Every relationship to it is
recorded in the [parity ledger](v3-upstream-parity-ledger.md) with
direct citations into that exact commit; no v3 claim delegates to any
other upstream document, revision, or intermediate record.

## System at a glance

One **Runtime** owns shared root state, the Registry, the stores, the
identity sources, and one permanent root Fiber. A Runtime begins with
its root Context, and every Context derived from that root remains a
view into the same Runtime. **Plugin** behavior is applied by spawning a
**Fiber**; a **FiberHandle** is the consumer's control handle to one non-root
Fiber. [CONTEXT.md](../CONTEXT.md) defines these terms; this section
fixes how the identities relate.

Six relations are deliberately independent — the independence of
residency, spawn origin, lifecycle ownership, and FiberHandle control is the
decision of
[ADR 0028](adr/0028-fiber-generations-own-cleanup-runtime-owns-residency.md):

| Relation | What it is | What it never implies |
| --- | --- | --- |
| Plugin behavior | The reusable behavior or declaration object applied by spawning a Fiber | The live identity of anything running |
| Fiber lifecycle | The era-local live identity owning one allocation's state and cleanup obligations | Residency, spawn provenance, or consumer control |
| FiberHandle control | A cloneable consumer handle referring to one non-root Fiber | Fiber lifetime: dropping every FiberHandle changes neither residency nor lifecycle |
| Registry residency | The Runtime and Registry keeping a Fiber strongly resident and addressable | Lifecycle ownership or public topology |
| spawn origin | The recorded Context from which a spawn was initiated | Parenthood, teardown responsibility, Scope ancestry, Event ancestry, or axis coupling |
| lifecycle ownership | Within a Fiber, each apply generation's ownership of the cleanup obligations that committed while its gate was open; across Fibers, explicit consumer composition of delivered FiberHandles | Any core cross-Fiber owner: ordinary spawn creates no parent cascade |

Core has six deep semantic owners — Context view, Registry, Fiber
lifecycle, Service/dependency coordination, Event dispatch, and
Logger — each hiding protocol topology behind complete semantic
operations and immutable, topology-free projections (see
[Module and crate seams](#module-and-crate-seams); the choice is owned
by [ADR 0036](adr/0036-module-and-crate-seams-are-semantic.md)).
Creation and era replacement are protocol slices of the Fiber lifecycle
owner; dependency indexing and Event storage are private implementation
slices, not peer public owners.

## Context views and derivation

A Context is a cheap immutable view into one Runtime, selecting a
current Fiber plus three independent axes — **isolate**, **Scope**, and
**intercept**. There is no general Context hierarchy: two views with
the same Runtime, Fiber, and semantically equal axis positions behave
identically regardless of derivation history, and no subsystem infers
ownership, authorization, Service fallback, Event routing, or lifecycle
meaning from derivation ancestry.

- **isolate** resolves each Service to one exact service realm: a fresh
  private realm separates the selected Service, and an explicit equal
  realm joins only that exact slot. Isolation never renames a Service
  and introduces no fallback, Service inheritance, Event reachability,
  or Context ancestry.
- **Scope** alone defines rooted Event reachability: scoped dispatch
  reaches ancestor-or-self registrations plus global registrations,
  siblings and descendants are excluded, and routing scoped to the
  root Scope differs from unscoped routing. Scope implies no
  lifecycle, Service,
  Fiber-parenthood, authorization, or Context relation.
- **intercept** composes Service-owned prepared configuration layers
  outer-to-inner under the relevant Service contract; it is immutable
  and is neither Event middleware nor authorization, routing, or
  Service placement.

Each primitive derivation changes exactly one axis and never starts
lifecycle work. Root derivation stays in the same Runtime and resets
Fiber attribution and all three axes. The complete matrix for every
multi-axis operation — no operation acquires an axis change it does
not name:

| Operation | Fiber | isolate | Scope | intercept / apply input |
| --- | --- | --- | --- | --- |
| Runtime creation | permanent root Fiber | Runtime default mapping | root Scope | empty/root intercept |
| primitive derivation | unchanged | only the isolate operation changes it | only the Scope operation changes it | only the intercept operation changes it |
| root derivation | permanent root Fiber | reset to Runtime default | reset to root Scope | reset to empty/root |
| new-Fiber spawn | fresh Fiber | inherit spawning Context | fresh child of spawning Scope | inherit Context intercept; compute fresh effective input |
| restart | same Fiber | unchanged | unchanged | reapply current committed input; preserve resolved edges |
| same-Fiber update | same Fiber | unchanged | unchanged | change only explicitly accepted committed input; no implicit retarget |
| era swap | fresh Fiber | inherit captured spawn origin and resolve anew | fresh child of captured origin Scope | reconstruct captured creation recipe and effective spec |

Axis orthogonality and this matrix are the decision of
[ADR 0032](adr/0032-context-axes-are-orthogonal.md). The exact
construction and derivation contracts live in the
[interface inventory](v3-public-interface.md#context-and-its-axes).

## Fiber lifecycle

A Fiber is era-local: restart and same-Fiber update preserve it; era
replacement ends it and creates a fresh one. Its states are `Loading`,
`Active`, `Pending`, `Unloading`, `Failed`, and `Disposed`. `Failed` is
published only after the failed generation's rollback completes;
`Disposed` only after terminal cleanup. A Fiber is created `Loading`,
becomes `Active` when its apply completes against a fully assigned
target, rests at stable `Pending` while its target has missing
assignments, passes through `Unloading` on its way down, and reaches
`Disposed` at the terminal barrier.

Within a Fiber, each apply generation owns every cleanup obligation
that commits while its gate is open; `Loading` and `Active` admit
generation-owned resources, while stable `Pending`, stable `Failed`,
closing, and `Disposed` reject them before publication or task start.
The root Fiber's generation remains open for Runtime duration. Restart
and same-Fiber update close and replace the generation together with
every obligation it still owns; ordinary spawn does not. This ownership
rule is the decision of
[ADR 0028](adr/0028-fiber-generations-own-cleanup-runtime-owns-residency.md).

One logical arbiter per Fiber serializes lifecycle intents: at most one
lifecycle operation advances on a Fiber at a time, accepted restart and
update intents serialize in admission order, provider drift coalesces
to the latest target, and terminal closing prevents later uncommitted
intents. The arbiter is logical ownership of the lifecycle transaction,
never an OS mutex held across user work or arbitrary awaits.

A Fiber settles and converges against its SemanticTarget through the
three named settle passes — initial, convergence, and restart — each
driving the Fiber toward its current target and out again through the
release-time drift recheck, which re-reads the live target before
publishing idle so provider/apply races converge without concurrent
duplicate apply. The ready operation drives or awaits convergence to
the current live SemanticTarget; the wait_state operation passively
observes reliable state publication and never drives target
settlement. A Fiber whose own
re-apply fails parks `Failed` against its current target as its own
business; a notification resolving to the same effective input and
publication assignments does not retry it, a genuinely changed
assignment or input permits another pass, and an explicit restart may
bypass same-target parking.

Spawn admission is generation-scoped but non-owning. Before allocation,
cancellation has no lifecycle effect. Once allocation and publication
create rollback responsibility, framework-owned work either delivers
the FiberHandle or terminally disposes and unlinks the undelivered Fiber; the
caller receives a FiberHandle only for a live quiescent `Active` or stable
`Pending` Fiber, and a failed or abandoned creation leaves no resident
attempted Fiber.

Restart and same-Fiber update preserve Fiber identity — the FiberId and
the resolved era-local dependency edges — and a committed update is
forward-only: it retains its new authoritative input value even when
the postcommit apply fails and the target parks `Failed`. Era
replacement is the identity-breaking counterpart: it fully ends the old
Fiber before attempting at most one fresh successor with a fresh
FiberId, fresh edges, a fresh sibling-era Scope, and fresh publication
occurrences, then converges affected dependents to their final targets
before delivering the fresh FiberHandle or reporting a closed incomplete
result. That transaction is the decision of
[ADR 0030](adr/0030-era-replacement-ends-one-fiber-before-creating-another.md);
the operation contracts live in the
[interface inventory](v3-public-interface.md#fiber-identity-lifecycle-and-typed-group-removal).

## Resources and cleanup

The Effect seam is registration-only: each resource family is set up by
its owning module and commits exactly one cleanup obligation into the
registering Context's current open generation. The retained families
are Service publications, listener registrations, exporter
registrations, timer operations, cooperative tasks, and pure effects.
Using a resource from another Fiber never transfers cleanup ownership:
the resource is removed or cancelled when its registering generation
closes.

Ownership transfer is atomic through gated publication, the one proven
nested bookkeeping seam: the commit publishes the resource occurrence
and its matching generation-owned cleanup together with every
correctness-required durable follow-up, and refusal publishes nothing
and leaves rollback with the resource owner. A publish-versus-close
race has exactly two outcomes — either the generation close wins and no
resource or cleanup control is delivered, or the commit wins and
exactly one generation-owned cleanup later removes or cancels the
resource. There is no third "visible but unowned" state. This seam and
its single nesting direction are fixed by
[ADR 0029](adr/0029-lifecycle-commits-complete-and-critical-sections-are-closed.md).

Manual control is exact. Each registration returns a move-only
capability with inert Drop whose consuming dispose or disarm competes
with the automatic generation cleanup for one exact claim per
obligation. A winning dispose runs the cleanup once — completing under
framework ownership despite caller cancellation and reporting a
returned error or panic as an opaque normalized failure without
restoring the occurrence — while a winning disarm releases the
obligation without running it. If generation cleanup wins, the consumed
manual operation reports the lost claim and never repeats cleanup.

Generation close claims every remaining obligation and drains them
sequentially in strict reverse-commit order — on unload, on restart or
update replacement, on failed-apply rollback, and on disposal. The
drain is attempt-all: a failing or panicking cleanup is isolated to its
own item, reported as a secondary diagnostic, and never prevents the
drain from reaching its lifecycle barrier.

Cooperative task registration is generation-owned: registration
precedes task start, refusal (inactive generation or executor
unavailable) starts nothing, and the task remains generation-owned
after caller cancellation. The task consumes its own
output inside the task, and its task-owned cleanup drains in the same
LIFO order before the contained, reported drain-join. Exact contracts
live in the
[interface inventory](v3-public-interface.md#effects-and-tasks).

## Registry residency

The Registry owns strong residency: from allocation until terminal
unlink it keeps every admitted Fiber strongly resident and
addressable. It carries two Registry-local vocabulary terms, defined
here and in the glossary's
[Registry-local section](../CONTEXT.md#registry-local-architecture-vocabulary):
**PluginKey** is an opaque input identifying plugin grouping
equivalence — typed Plugins group by Rust type identity and erased
Plugins receive an anonymous key — and **PluginGroup** is the current
Registry-private residency allocation selected by a PluginKey. A
PluginGroup may be pruned and recreated; it is neither an interface, a
lifecycle owner, nor a stable plugin-instance identity, and it carries
no per-Fiber facts, stable seat, or public topology.

Residency bookkeeping is exact-allocation. Attach and detach are
atomic; final disposal releases the exact residency occurrence and
unlinks it; and the conditional prune removes a mapping only if it
still names that exact empty allocation, so an old or detached unlink
can never delete a recreated group.

Bulk removal — one typed grouping-level operation — commits at
detaching one current PluginGroup allocation and freezes its completion
set: a Fiber attaching before the detach joins the set and is fully
disposed, while one attaching after it creates or joins a fresh current
allocation outside the removal. After the commit, framework-owned work
disposes every frozen member through the ordinary terminal barrier with
no promised inter-Fiber order; absence is success, later removals see
only their then-current allocation, and caller cancellation does not
stop the drain. The removal refuses a self-wait from a member's settle
context before any detach.

The Registry is never a consumer capability: consumers receive
topology-free observation and the grouping-level removal operation, and
nothing else. Residency implies no lifecycle ownership, spawn origin
implies no teardown, and dropping every FiberHandle changes neither — the
four-way independence of [System at a glance](#system-at-a-glance).

## Services and dependency convergence

Service identity, exact realm placement, and provider-publication
occurrence are three distinct things — the convergence rule is the
decision of
[ADR 0031](adr/0031-service-convergence-tracks-exact-publication-assignments.md).
A Service is a Runtime-local named typed value contract: within one
Runtime, one name denotes one compatible contract. An exact slot is the
pair (Service, service realm) selected by the Context's isolate axis,
and a slot has at most one eligible current occurrence. A provider
publication is one successful publication occurrence by one Fiber
generation into one exact slot; exact occurrence identity — not Service
identity — governs visibility, replacement, stale-cleanup safety,
mutation, removal, and dependency assignment.

Visibility follows actual publication, never declared metadata. A
`Loading` generation may occupy a slot but is invisible: lookup and
dependency assignment treat the slot as missing until the publication
reaches `Active`. Generation gate close withdraws visibility before
cleanup runs. Replacement is exact: a later generation may occupy the
slot over a closed occurrence, and stale cleanup from the old
generation can neither remove nor shadow the replacement. Lookup
addresses exactly the Context-selected realm with no fallback or
ancestry, requires no declaration membership, and reports an
incompatible same-name contract as mismatch rather than absence. Only
actual visibility changes create dependency drift: declared provide
metadata, `Loading` installation, a same-occurrence payload mutation,
stale cleanup, and storage relocation change no target.

Fibers own the authoritative dependency edges. At creation, each
required Service resolves exactly once through the spawning Context's
isolate mapping into a fixed era-local exact slot, and the final
dependency declaration is one normalized required row per Service after
Plugin declarations and Loader overlay. These edges are immutable for
the era: restart and same-Fiber update preserve them, and era
replacement resolves a fresh set from the captured creation recipe. No
store, index, or Registry record is the authority for these
relationships.

A Fiber settles against its SemanticTarget: the committed effective
apply input plus every exact dependency slot, each assigned missing or
one exact visible provider publication occurrence. Target equality is
semantic: it includes committed apply-relevant configuration and
excludes store, index, counter, ordering, wakeup, and value identity —
so mutating the payload of the same occurrence does not change the
target, while removing an occurrence or providing a new one does.
Failed-target parking, retry eligibility, and ready convergence follow
from this equality.

Every effective visibility mutation atomically commits a durable target
recheck for the complete affected set — deduplicated per Fiber — in the
same semantic commit, and the settlement kicks that follow run outside
locks. Durable recheck is protocol state, not executor or observer
availability: a missing executor may skip a kick but cannot lose drift,
the settlement release rechecks the live target before publishing idle,
and a later ready operation drives any drift that remains. The race law is
closed: edges fixed before the mutation receive a durable recheck,
edges fixed after it see the current state in their initial settle, and
unregistered or disposed dependents owe no progress.

DependencyIndex is replaceable acceleration, never semantic authority:
a derived projection of the Fiber-owned edges that accelerates
affected-dependent discovery. Disabling it, rebuilding it, changing its
order, or replacing it with authoritative scanning changes cost only;
pending diagnostics, SemanticTargets, affected-set membership, and
settlement outcomes are unchanged. The exact lookup, publication, and
control contracts live in the
[interface inventory](v3-public-interface.md#service-publication-and-lookup).

## Events, update control, and observation

An Event is one Runtime-local named typed contract over its invocation
and result behavior: within one Runtime, one name binds one compatible
contract, and listener registrations are independent occurrences, not
Event identity. Exactly four completion-aware primitives exist —
awaited ordered notification, awaited parallel notification, sequential
query with explicit miss/answer outcomes, and the owned waterfall with
a mandatory tail — each taking an explicit routing, and the
waterfall-query composition is derived, propagating that routing, not a
fifth primitive. The
dispatch algebra and its removals are the decision of
[ADR 0033](adr/0033-events-are-typed-completion-aware-and-occurrence-claimed.md).

Listener registrations are exact generation-owned occurrences committed
through the gated seam. Listener roles — Observer, Responder, Mapper,
and Around — are semantic protocol input, preflighted for compatibility
before any claim; a known incompatible snapshot fails before callbacks,
claims, or state factories run. Delivery claims the occurrence before
invocation, so removal and once consumption compete for one exact
claim: remove-before-claim skips future delivery, claim-before-remove
completes the owned work, and a once occurrence is consumed atomically
immediately before its winning callback — never by preflight failure,
short circuit, outer veto, or a lost claim. Callback Context
attribution comes from registration, never from the emitter's routing
position. Every primitive is completion-aware: its future is never
silently detached, and completion awaits all work it already owns, with
listener failures contained and normalized exactly once.

Update control is not an Event. Same-Fiber update policy is a typed,
framework-invoked Mapper/Around pipeline scoped to the target Fiber
over a sealed, move-only, one-attempt candidate. Consumers may register
policy but can never dispatch it: only the framework's update operation
runs the pipeline, its mandatory private tail performs no lifecycle
commit, and admission is revalidated after control completes and before
the commit. Veto or precommit failure leaves the old configuration and
generation intact; postcommit apply failure is lifecycle business the
pipeline cannot observe or recover; era replacement never invokes the
pipeline. This separation is the decision of
[ADR 0034](adr/0034-update-control-is-precommit-and-non-dispatchable.md);
the exact registration and outcome contracts live in the
[interface inventory](v3-public-interface.md#typed-update-control).

Runtime observation follows protocol truth and never drives it — the
decision of
[ADR 0035](adr/0035-runtime-observation-follows-protocol-truth.md). The
ordering is fixed: authoritative commit, then durable protocol signal,
then lock release, and only then optional observation delivery.
Deleting every observer leaves lifecycle, settlement, visibility,
cleanup, and completion outcomes identical. The observation stream
carries exactly five immutable record categories — Fiber residency,
Fiber state, Service visibility, listener registration, and primitive
dispatch completion — correlated by opaque identities and delivered
Runtime-wide, detached, best-effort, parallel, and attempt-all, with
delivery generation-owned by the registering Context and narration of
its own dispatch suppressed. Observation holds no veto, rollback,
progress, completion, ordering, audit, retention, or replay authority.
The complementary flat Runtime snapshot is an unordered, read-only
current view whose records are individually self-consistent without
being a globally linearizable instant. Exact record shapes live in the
[interface inventory](v3-public-interface.md#runtime-snapshots-and-observations).

## Concurrency constitution

Every deep lifecycle transaction names exactly one irreversible commit.
Cancellation before that commit changes no framework state — no state,
configuration, gate, claim, allocation, publication, or observation.
Cancellation after it abandons only the caller's wait while
framework-owned work continues independently of caller polling until it
reaches the operation's documented barrier. Runtime-agnostic lifecycle work
normally stays on the current executor and transfers its same pinned future to
Cordis's shared completion runtime only if executor shutdown drops it after a
normal `Pending`; arbitrary async effect cleanup starts on that completion
runtime from its first poll. This uniform law and its supporting rules are the decision of
[ADR 0029](adr/0029-lifecycle-commits-complete-and-critical-sections-are-closed.md).

| Operation | Irreversible commit | Required independent completion |
| --- | --- | --- |
| Fiber disposal | Open-to-Closing terminal claim | cleanup, Disposed, unlink/prune |
| Registry removal | detach current PluginGroup allocation | dispose every detached Fiber |
| restart | irreversible closure/replacement of the old generation | current-target quiescence |
| update | new config plus generation-replacement commit | committed-config target quiescence |
| era swap | source swap claim | old disposal, successor outcome/cleanup, final convergence |
| new-Fiber creation | allocation/publication requiring rollback responsibility | FiberHandle delivery or undelivered-Fiber disposal |

The same law governs the two remaining deep operations: a winning exact
manual cleanup dispose commits at its claim and completes under
framework ownership despite caller cancellation, and Loader result
handoff commits as each FiberHandle is obtained — abandoned handoff disposes
already-obtained FiberHandles in reverse success order, attempt-all, through the
same shutdown-resilient completion seam, while dropping a delivered outcome is
inert. Disposal itself coalesces: the
first Open-to-Closing claim commits, and all concurrent or later
disposals share the same completion.

Framework critical sections protect bookkeeping only. User callbacks,
conversions, re-entrant clones, future polling, awaits, task spawn,
lifecycle work, notification, rollback, and potentially last-reference
destruction all happen after synchronization is released: values are
extracted under lock and destroyed afterward — two-phase
extraction/drop — on success, refusal, stale no-op, rollback, and error
alike. No lock guard crosses an await or poll boundary, and there is no
global lock hierarchy. Gated publication is the one narrow proven
nested bookkeeping seam, with a single permitted direction — recheck
the generation gate, then atomically commit occurrence plus cleanup —
calling no user or re-entrant work and releasing before notification,
destruction, rollback, or lifecycle work.

Reliable protocol signals, not executor availability or observer
delivery, own lifecycle truth, target drift, operation completion, and
waiter wakeups: a missing executor may skip observations and kicks but
cannot lose a committed change, and the next capable lifecycle driver
converges it.

Lifecycle self-waits are refused before any effect by attribution to
the concrete Fiber allocation rather than to any public id or state.
The typed recursion refusal names the operation and the FiberId and
covers ready, wait_state, restart, update, era swap, dispose, and typed
group removal — the last refused before any Registry detach — while
legal unrelated-Fiber waits and the dynamic era-swap backstops are
preserved. Raw Tokio spawns intentionally start without task-local settle
attribution; `Context::spawn_attributed` is the explicit user-task seam for
subtasks that remain inside the settle dependency graph, with transferred
frames expiring when the source scope exits.

## Module and crate seams

Core hides protocol topology behind six deep semantic owners. Each
exposes complete semantic operations and immutable topology-free
projections through its external seam, and internal seams carry only
the narrow complete capability one owner requires from another. Locks,
storage rows, allocation pointers, claims, counters, and partial
transaction phases never cross either kind of seam. Rust modules are
private by default behind curated semantic facades: every public path
has consumer-meaningful semantic identity rather than mirroring source
layout. This seam policy is the decision of
[ADR 0036](adr/0036-module-and-crate-seams-are-semantic.md), and the
interface policy the facades follow is the decision of
[ADR 0037](adr/0037-public-interfaces-expose-semantics-not-representation.md).

| Owner | Complete responsibility | Deliberately excluded |
| --- | --- | --- |
| Context view | Immutable Runtime association, current-Fiber attribution/control capability, orthogonal isolate/Scope/intercept positions, derivation and root reset, frozen creation views, Scope reachability, exact Service-to-realm mapping, and ordered intercept layers | Fiber lifecycle, Service semantics, Event dispatch, config composition, textual isolate-label policy, and Runtime composition storage |
| Registry | Residency membership and topology-wide mutation, exact-allocation attach/release/prune arbitration, topology-wide enumeration, committed bulk-removal completion sets, and removal-barrier orchestration | Consumer control, dependency interpretation, lifecycle state, and storage-shaped observation |
| Fiber lifecycle | Fiber and generation identity/state, lifecycle arbitration and transactions, authoritative era-local dependency edges, apply/target state, settlement and convergence, creation and era replacement, disposal, cleanup ownership, and recursion refusal | Registry topology, Service projection invariants, Event dispatch semantics, and resource-specific publication invariants |
| Service/dependency coordination | Service visibility mutation, exact changed-slot batching, derived dependency projection, affected-dependent discovery, durable mutation-to-drift commit, and outside-lock settlement kicks | Authoritative dependency relationships, SemanticTarget construction, lifecycle arbitration, and settlement |
| Event dispatch | Named typed contracts, listener occurrence lifecycle, routing, ordering, claims, all dispatch/composition modes, invocation, containment policy, and completion | The authoritative lifecycle, Service, observation, or update transaction that requested dispatch |
| Logger | Runtime-local diagnostic sequencing, channels and filtering, immutable records, exporter occurrences, routing/fan-out, per-exporter isolation, and recursion-safe fallback | Ordinary Service availability, durable retention, default buffering, rich presentation, and global failure policy |

Context is an access carrier, not a service locator for owner
internals. Creation and era replacement are protocol slices of the
Fiber lifecycle owner; dependency indexing and Event storage are
private implementation slices. None of these is a peer public owner.

The package dependency graph is exactly:

```text
cordis-loader -> cordis-core <- cordis-timer
```

`cordis-core` remains free of `serde`, `serde_json`, and production
Tokio-time dependencies. Loader owns serde and declarative/config
adaptation; Timer owns Tokio-time integration. Neither leaf exposes its
policy or representation through core, and core consumers never acquire
the leaves' dependencies. Deleting either leaf leaves core behavior
unchanged. The exact facade inventories live in the
[interface inventory](v3-public-interface.md#crate-facades-and-canonical-paths).

## Optional leaf crates and the foundation Logger

**Loader** (`cordis-loader`) is an optional caller-held declarative
operation module — not a Runtime Service and not a long-lived
self-updating owner. It owns declarative identity, document
deserialization, validation, enablement, reachability, deterministic
traversal, and outcome correlation behind an immutable, validated,
reusable LoadPlan whose EntryIds are stable across clones of one plan
lineage. Structural parentage determines execution sequencing only —
never Fiber parenthood, Service realm, Scope ancestry, Registry
grouping, or lifecycle ownership. The external synchronous resolver is
the seam for all target-specific preparation: it receives only resolve
key, JSON configuration, and inject syntax, and returns a fully
prepared, overlaid, and sealed prepared Plugin input, an unknown-key
absence, or a typed failure, so an adaptation failure creates no Fiber,
residency, dependency projection, or generation. Realm-label policy is
Loader's per-execution choice; core sees only opaque Runtime-local
realms. Load is partial, not transactional, and yields exactly one
ordered outcome per plan entry; successful FiberHandle delivery is the
handoff that ends Loader ownership, with reverse-success-order
attempt-all rollback of already-obtained FiberHandles if handoff is abandoned.
The exact schema, resolver, and outcome contracts live in the interface
inventory's [plan](v3-public-interface.md#loader-plan-and-source-schema)
and
[resolver/outcome](v3-public-interface.md#loader-resolver-execution-and-outcomes)
sections.

**Timer** (`cordis-timer`) is an optional flat leaf capability crate
owning complete time operations and all Tokio-time integration: a
complete fallible sleep operation, a work-owning timeout operation, and
a fixed-phase interval operation.
Its only core seam is the store-free operation that binds an
already-prepared cleanup obligation into the current generation: core
lifecycle decides when the generation-owned obligation is claimed, and
Timer decides what claiming that timer-specific obligation means.
Deadline and cancellation machinery remains private to Timer, and core
stays production-Tokio-time-free. The exact operation contracts live in
the
[interface inventory](v3-public-interface.md#timer-facade-and-operations).

**Logger** is a core foundation owner, not a Service: Runtime-local
diagnostic sequencing, semantic severity order, named channels,
filtering, immutable records, exact exporter registrations, and
outside-lock attempt-all fan-out with per-exporter isolation and a
recursion-safe fallback. Exporter failure or reentrancy can never
affect Runtime progress. Logger provides an opt-in bounded buffer
exporter and no default retention, durable history, audit ordering, or
global failure policy. The exact contracts live in the
[interface inventory](v3-public-interface.md#logger).

## Deliberate baseline absences and reopening conditions

The baseline deliberately excludes capabilities that no demonstrated
consumer need justifies. Each absence below is part of the approved
target, not an oversight:

- no structured core lifecycle owner or FiberGroup, and no parent
  cascade: consumers compose delivered FiberHandles explicitly, including
  reverse-spawn-order Harness teardown;
- no Runtime-wide shutdown;
- no public Registry or storage-shaped observation;
- no `List` and no generic acquire algebra;
- no Loader Service or self-update loop, no realm fallback, and no
  global or core textual realm labels;
- no durable observation journal, retention, or replay;
- no public ArmedSleep-style race transport, no throttle/debounce, and
  no default Logger buffer; and
- no stable plugin seat, `Lifetime`, `DisposalScope`, or general
  Context hierarchy — the glossary's
  [language exclusions](../CONTEXT.md#language-exclusions) are
  architectural absences.

An absence stands unless its focused falsifier graduates a new
decision. The resolved reopening conditions are exactly:

- an admitted child creation that core must collectively drain before
  its FiberHandle has been delivered — the only condition that graduates a
  Runtime-local structured owner, never keyed on grouping equivalence,
  spawn origin, or Fiber identity;
- a concrete consumer requiring deterministic teardown of every
  resident Fiber and root registration — the only condition that
  graduates Runtime-wide shutdown, with independent admission,
  detach-all, ordering, executor, cancellation, and completion
  decisions;
- a demonstrated named-realm rendezvous that opaque Runtime-local
  realms cannot express;
- a demonstrated same-Fiber dependency retargeting need, which era
  replacement deliberately does not serve;
- a demonstrated non-tree Event audience that rooted Scope
  reachability cannot express; and
- a concrete consumer requiring reliable retained audit or replay of
  Runtime transitions — the falsifier named by
  [ADR 0035](adr/0035-runtime-observation-follows-protocol-truth.md),
  which graduates a separate durable-journal module rather than
  weakening the best-effort stream.

No capability is retained merely because a previous implementation or
the pinned upstream contains it.

## Decision index

The twelve independent hard-to-reverse decisions of the v3 architecture,
each in one accepted ADR, linked by title:

1. [Fiber generations own cleanup; Runtime owns residency](adr/0028-fiber-generations-own-cleanup-runtime-owns-residency.md)
2. [Lifecycle commits complete independently and critical sections are closed](adr/0029-lifecycle-commits-complete-and-critical-sections-are-closed.md)
3. [Era replacement ends one Fiber before creating another](adr/0030-era-replacement-ends-one-fiber-before-creating-another.md)
4. [Service convergence tracks exact publication assignments](adr/0031-service-convergence-tracks-exact-publication-assignments.md)
5. [Context has orthogonal isolate, Scope, and intercept axes](adr/0032-context-axes-are-orthogonal.md)
6. [Events are typed, completion-aware, and occurrence-claimed](adr/0033-events-are-typed-completion-aware-and-occurrence-claimed.md)
7. [Update control is precommit and non-dispatchable](adr/0034-update-control-is-precommit-and-non-dispatchable.md)
8. [Runtime observation follows protocol truth and never drives it](adr/0035-runtime-observation-follows-protocol-truth.md)
9. [Module and crate seams are semantic](adr/0036-module-and-crate-seams-are-semantic.md)
10. [Public interfaces expose semantics, not representation](adr/0037-public-interfaces-expose-semantics-not-representation.md)
11. [Plugin input names role; Prepared wrappers name stage](adr/0038-plugin-input-names-role-prepared-wrappers-name-stage.md)
12. [Consumer Fiber control is a FiberHandle, not a Fork](adr/0039-consumer-fiber-control-is-a-fiber-handle.md)

Each ADR states its rule and rationale self-contained. Its
non-normative lineage block is historical evidence only: deleting every
lineage block, together with the optional migration document, leaves
the required-reading closure above — glossary, this document, the
interface inventory, these twelve decisions, and the parity ledger —
complete, with no historical or current-implementation dependency.
