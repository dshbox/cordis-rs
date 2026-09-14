# Module and crate seams are semantic

Status: accepted

Core hides protocol topology behind six deep semantic owners: Context
view, Registry, Fiber lifecycle, Service/dependency coordination,
Event dispatch, and Logger. Each owner exposes complete semantic
operations and immutable topology-free projections through its
external seam, and internal seams between owners carry only the
narrow complete capability one owner requires from another. Locks,
storage rows, allocation pointers, claims, counters, and partial
transaction phases never cross either kind of seam. Rust modules are
private by default behind curated semantic facades: every public path
has consumer-meaningful semantic identity rather than mirroring
source layout.

Context is an access carrier, not a service locator for owner
internals. Creation and era replacement are protocol slices of the
Fiber lifecycle owner; dependency indexing and Event storage are
private implementation slices. None of these is a peer public owner.

## Owner responsibilities and exclusions

| Owner | Complete responsibility | Deliberately excluded |
| --- | --- | --- |
| Context view | Immutable Runtime association, current-Fiber attribution/control capability, orthogonal isolate/Scope/intercept positions, derivation and root reset, frozen creation views, Scope reachability, exact Service-to-realm mapping, and ordered intercept layers | Fiber lifecycle, Service semantics, Event dispatch, config composition, textual isolate-label policy, and Runtime composition storage |
| Registry | Residency membership and topology-wide mutation, exact-allocation attach/release/prune arbitration, topology-wide enumeration, committed bulk-removal completion sets, and removal-barrier orchestration | Consumer control, dependency interpretation, lifecycle state, and storage-shaped observation |
| Fiber lifecycle | Fiber and generation identity/state, lifecycle arbitration and transactions, authoritative era-local dependency edges, apply/target state, settlement and convergence, creation and era replacement, disposal, cleanup ownership, and recursion refusal | Registry topology, Service projection invariants, Event dispatch semantics, and resource-specific publication invariants |
| Service/dependency coordination | Service visibility mutation, exact changed-slot batching, derived dependency projection, affected-dependent discovery, durable mutation-to-drift commit, and outside-lock settlement kicks | Authoritative dependency relationships, SemanticTarget construction, lifecycle arbitration, and settlement |
| Event dispatch | Named typed contracts, listener occurrence lifecycle, routing, ordering, claims, all dispatch/composition modes, invocation, containment policy, and completion | The authoritative lifecycle, Service, observation, or update transaction that requested dispatch |
| Logger | Runtime-local diagnostic sequencing, channels and filtering, immutable records, exporter occurrences, routing/fan-out, per-exporter isolation, and recursion-safe fallback | Ordinary Service availability, durable retention, default buffering, rich presentation, and global failure policy |

## The deletion test

A module or capability survives only against a demonstrated consumer
need. No Registry, storage, or claim machinery becomes public merely
because it exists internally, and no speculative utility module
enters the baseline merely because its implementation could be
useful. `List<T>` fails this test — no demonstrated consumer needs
the capability — so it is absent from the baseline and no utility
crate is introduced to preserve it. The same test keeps public
ArmedSleep-like race transport, throttle/debounce, default Logger
buffering, and backing Loader trees out of the baseline.

## One-way crate graph

The package dependency graph is exactly:

```text
cordis-loader -> cordis-core <- cordis-timer
```

`cordis-core` remains free of `serde`, `serde_json`, and production
Tokio-time dependencies. Loader owns serde and declarative/config
adaptation; Timer owns Tokio-time integration. Neither leaf exposes
its policy or representation through core, and core consumers never
acquire the leaves' dependencies.

### Loader firewall and handoff

`cordis-loader` is an optional caller-held declarative operation
module — not a Runtime Service and not a long-lived self-updating
owner. The core creation seam is a semantic firewall. Loader owns
declarative identity, representation and document deserialization,
validation, enablement, reachability, deterministic traversal, and
outcome correlation; the external resolver owns resolve-key
interpretation and every target-Plugin-specific config adaptation
into typed Plugin input, then seals it before lifecycle admission. All declarative
adaptation completes before the one complete core spawn call, so an
adaptation failure creates no Fiber, residency, dependency
projection, or generation. Loader owns textual realm-label policy
per execution; core creates only opaque Runtime-local service realms
and exact immutable Service-to-realm mappings. Successful FiberHandle
delivery is the handoff that ends Loader ownership: before outcome
delivery Loader still owns obtained FiberHandles and rolls them back in
reverse success order if handoff is abandoned; after delivery, FiberHandle
and outcome Drop are inert and the consumer owns composition.

### Timer seam over core cleanup

`cordis-timer` is an optional leaf capability crate owning complete
time operations and all Tokio-time integration. Its only core seam
is the store-free operation that binds an already-prepared cleanup
obligation into the current generation: core lifecycle decides when
the generation-owned obligation is claimed, and Timer decides what
claiming that timer-specific obligation means. Timer acquires no
second lifecycle state machine and sees no lifecycle internals or
cancellation transport; its ArmedSleep-like race machinery remains
private.

### Logger's foundation role

Logger is an independent Runtime-foundation owner inside core, not a
Service: it provides Runtime-local diagnostic sequencing, channels,
filtering, immutable records, exact exporter occurrences, and
attempt-all fan-out without ordinary Service availability, durable
retention, default buffering, rich presentation, or global failure
policy. Exporter failure or reentrancy is isolated per exporter and
can never affect Runtime progress.

## Rationale

Callers receive leverage without learning protocol topology: each
seam offers complete operations whose use never requires
reconstructing internal state. Maintainers retain locality: each
owner's invariants hold behind its own seam instead of folding every
participating invariant into a god module. Optional policies and
their dependencies do not infect core: serde stays in Loader,
Tokio-time stays in Timer, and core compiles and runs without
either.

## Consequences

- Deleting either leaf crate leaves core behavior unchanged; a
  consumer can compose Loader-equivalent or Timer-equivalent flows
  over the public core facade without touching internal seams.
- Registry is never a consumer capability: consumers obtain
  topology-free observation and a semantic grouping-level
  bulk-removal operation, never Registry or PluginGroup topology.
- Cross-crate visibility and testing convenience do not weaken the
  private-by-default rule.
- `List<T>` stays removed unless a demonstrated consumer need
  graduates a new decision; nothing in core, Loader, or Timer
  reintroduces it.

## Non-normative lineage

| V3 rule | Lineage classification | Historical evidence only |
| --- | --- | --- |
| Six deep semantic owners and narrow internal seams | collapses several historical ADRs into one final rule | ADRs 0010, 0013, 0015, 0016, 0022, 0023, 0024, and 0025 |
| Registry-hidden Runtime observation/removal and exact residency claims | supersedes part of an existing ADR | ADRs 0005 and 0025 |
| Caller-held optional Loader leaf | preserves an existing ADR | ADRs 0004 and 0005 |
| Loader preparation firewall, private topology, and Loader-owned realm-label policy | supersedes part of an existing ADR | ADRs 0005 and 0006 |
| Optional Timer leaf over generic lifecycle cleanup; ArmedSleep remains private | supersedes part of an existing ADR | ADRs 0004 and 0006 |
| Removal of `List<T>` from the baseline | supersedes part of an existing ADR | ADRs 0004 and 0012 |
| Consolidated v3 module/crate seam rule | supersedes part of an existing ADR | ADR 0026 |
