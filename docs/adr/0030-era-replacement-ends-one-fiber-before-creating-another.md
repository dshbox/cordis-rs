# Era replacement ends one Fiber before creating another

Status: accepted

Era replacement — the era swap — is the identity-breaking replacement of
one Fiber by a freshly spawned successor, and it always ends one Fiber
completely before creating another. A successful swap is one ordered
transaction:

1. **Preflight.** Compatibility, liveness, and recursion are checked
   over the source and every affected dependent known at entry; a call
   attributed to any member's settle context refuses before the source
   claim, and dependents discovered later repeat the allocation check
   before they are awaited.
2. **One live-source claim.** At most one swap claims a live source, and
   the claim irreversibly disposes the old Fiber. Losing swaps — and a
   swap losing to an ordinary dispose — refuse before any successor
   allocation. A later ordinary disposal waits only the old Fiber's
   barrier and neither follows nor owns the winner's successor.
3. **Complete old-Fiber death.** The old Fiber passes its full terminal
   barrier — generation drain, Disposed publication, exact Registry
   unlink, and conditional prune — and its publications are withdrawn
   before any successor visibility. Nothing crosses the replacement: the
   successor inherits no provider publication, listener registration,
   effect journal, or old Fiber Scope; surviving old-Scope descendants
   are not migrated; Fibers the old Fiber spawned are never cascaded;
   and no identity survives — the successor receives a fresh FiberId.
4. **At most one fresh successor.** The operation attempts one
   successor, replaying the captured creation recipe — the same Plugin
   behavior, the captured spawn origin, and grouping equivalence — with
   the newly supplied Plugin input, prepared before lifecycle admission, newly resolved exact
   dependency edges, a fresh sibling-era Scope beneath the captured
   spawn-origin Scope, and fresh publication occurrences.
5. **Final convergence.** Once the successor is quiescent Active or
   stable Pending, affected dependents are re-queried fresh — including
   dependents discovered mid-swap — and converged to their final current
   targets, not merely observed off the old era.
6. **Handoff.** Only then does the caller receive the fresh FiberHandle.

A swap called through a closing or Disposed FiberHandle refuses with `Closed`
before successor allocation or any dependent effect. Dead-FiberHandle respawn
does not exist: replacement requires a live source.

An incomplete result is equally closed. If successor creation is refused
before allocation, if a fresh successor is concurrently invalidated
before FiberHandle handoff, or if an allocated successor's apply fails or
panics, the outcome is the same closed middle state: the old Fiber is
gone, no attempted successor remains resident, every attempted-successor
cleanup has completed, and affected dependents have converged to their
final targets before the operation returns. A failed successor's Plugin
failure remains the primary reported cause of the
`EraSwapError::Incomplete` outcome, and a retained Failed successor does
not exist.

Era replacement obeys the commit-or-no-effect law: cancellation before
the source claim changes nothing, and cancellation after it abandons
only the wait while source disposal, successor success-or-cleanup, final
dependent convergence, and the handoff-or-no-resident guarantee complete
under framework ownership. Caller cancellation is never reported as
successor loss.

## Rationale

Identity and failure postconditions stay unambiguous under racing swaps,
ordinary disposal, provider replacement, and cancellation. Because the
old era's death is complete before any birth and at most one successor
is ever attempted, exactly one resident outcome is possible — the fresh
successor quiescent, or nothing — and rollback to an already-ended era
is impossible: the honest middle state is reported instead of a
resurrected old Fiber or a parked Failed successor.

## Consequences

- No stable seat survives a swap. No group allocation, Scope node, or
  publication occurrence carries over, and an old Scope node that
  remains referenced after its Fiber dies does not preserve the Fiber or
  create a stable event seat.
- Multiple racing successor attempts do not exist: exactly one swap owns
  the source claim, and every loser refuses before allocation.
- Dependents converge to their final targets before return in success
  and in every incomplete case; a dependent whose own re-apply fails
  parks Failed against its current target as its own business, never as
  a swap-corrupting state.

## Non-normative lineage

| V3 rule | Lineage classification | Historical evidence only |
| --- | --- | --- |
| One live-source era claim, old-Fiber death, fresh successor, complete failure cleanup, and final-target convergence | supersedes part of an existing ADR | ADR 0021 |
