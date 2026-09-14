# Fiber generations own cleanup; Runtime owns residency

Status: accepted

Within a Fiber, each apply generation owns every cleanup obligation that
commits while its gate is open. Every retained resource family — Service
publications, listener registrations, exporter registrations, timer
operations, cooperative tasks, and pure effects — commits exactly one
cleanup obligation to the registering Context's current open generation,
and that generation remains the obligation's only owner for its whole
life. Using a resource from another Fiber never transfers cleanup
ownership: the resource is removed or cancelled when its registering
generation closes, regardless of who used it. Restart and same-Fiber
update close and replace the generation and with it every obligation the
old generation still owns; ordinary spawn does not.

Cleanup ownership is the only ownership core has. Registry residency,
spawn origin, lifecycle ownership, and consumer-held FiberHandle control are
four independent relations:

- **Residency** is the Runtime and its Registry keeping each admitted
  Fiber strongly resident and addressable from allocation until terminal
  unlink. Residency implies no lifecycle ownership, and dropping every
  FiberHandle changes neither residency nor lifecycle.
- **Spawn origin** records the Context from which a spawn was initiated.
  It is provenance and supplies the spawning Context for derivation; it
  implies no parenthood and no teardown responsibility. A spawned Fiber
  may outlive its spawn origin, and disposing an origin never cascades
  to spawned Fibers.
- **Lifecycle ownership** ends at the generation boundary. Ordinary
  spawning establishes residency and a spawn origin but no
  Fiber-to-Fiber ownership, so no Fiber is responsible for ending
  another Fiber.
- **FiberHandle control** is explicit consumer control over one non-root Fiber.
  Consumers compose delivered FiberHandles themselves: the Harness Roster
  records FiberHandles in spawn order and disposes them in reverse spawn order,
  attempt-all, as consumer policy — never a core ownership relation.

The root Fiber and its generation are permanent for the Runtime's whole
duration. Dropping Context or FiberHandle clones never initiates root or
Runtime teardown; root registrations persist unless their own exact
control claims or disarms them. Core has no structured owner and no
Runtime shutdown: no FiberGroup owner, no stable era seat, and no
implicit all-resident teardown.

## Rationale: the deletion test

Generation ownership plus Registry residency plus consumer FiberHandle
composition satisfies every demonstrated cleanup and teardown need with
no stable owner or seat, no cross-Fiber ownership transfer, and no
second cleanup axis. Each candidate ownership abstraction was deleted
against demonstrated need:

- **FiberGroup owner.** The only demonstrated group need is typed bulk
  removal, and Registry detach plus the ordinary per-Fiber terminal
  barrier already serves it. A PluginGroup stays a private, replaceable
  residency allocation; an owner object would add a stable seat without
  adding a demonstrated capability.
- **`Lifetime` + `DisposalScope`.** A scope object owning cleanup beside
  the generation duplicates the generation's journal as a second cleanup
  axis. Every demonstrated obligation already commits to the generation
  gate with one exact claim.
- **Standalone cleanup scope.** A scope detached from any Fiber
  lifecycle has no lifecycle barrier to drain against; no demonstrated
  consumer needs framework-owned cleanup that outlives its owning
  generation.
- **Combined owner/scope.** Composing the two deleted abstractions
  inherits both failure modes — cross-Fiber transfer questions and a
  second cleanup axis — while serving no additional demonstrated need.
- **Parent cascade.** Demonstrated needs run the other way: a spawned
  Fiber must outlive its spawn origin and survive the origin's disposal.
  Cascade-shaped teardown is consumer policy — reverse spawn-order
  Roster disposal — not a core relation.

## Falsifier

This decision falls if core must collectively drain an admitted child
before its FiberHandle has been delivered: such a drain would require a core
ownership relation over admitted Fibers that this ADR forbids.

## Non-normative lineage

| V3 rule | Lineage classification | Historical evidence only |
| --- | --- | --- |
| Apply-generation cleanup ownership; residency, origin, and ownership remain distinct | collapses several historical ADRs into one final rule | ADRs 0003, 0004, and 0023 |
| No structured core owner; Harness retains returned FiberHandles | preserves an existing ADR | ADRs 0005 and 0011 |
| No parent cascade, Lifetime, DisposalScope, or stable era seat | preserves an existing ADR | ADRs 0021 and 0023 |
| Permanent root with no implicit Runtime shutdown | preserves an existing ADR | ADRs 0003 and 0005 |
