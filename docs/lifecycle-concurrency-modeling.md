# Lifecycle concurrency modeling plan

Status: active working plan — umbrella issue #99

This document is the durable work log and acceptance boundary for systematic
concurrency validation of Cordis v3 lifecycle arbitration. It is deliberately
not an ADR: model exploration may falsify assumptions recorded here. Accepted
semantic decisions remain authoritative in `CONTEXT.md` and `docs/adr/`.

## Why this work exists

Cordis already has race-oriented unit/integration tests, cancellation contracts,
trybuild surface contracts, and explicit lifecycle ADRs. The remaining high-risk
area is the hand-written concurrency protocol at the seams between:

- Fiber convergence arbitration (`crates/cordis-core/src/fiber/inertia.rs`),
- Service-target revision accounting and `FiberHandle::ready()`, and
- Era replacement arbitration (`crates/cordis-core/src/fiber/era.rs`).

Issue #96/#97 made detached framework-task invariant panics observable without
recovering or synthesizing lifecycle state. That solves diagnosis after a
framework invariant breaks; this work asks the more upstream question: which
scheduler interleavings can break those invariants during otherwise healthy
execution?

The goal is **correctness evidence**, not "add Loom". Loom is the first tool to
prototype for the small atomic protocol; Shuttle remains a candidate for larger
async schedules. Real Tokio contract tests remain required either way.

## Corrected vocabulary

Do not use `IDLE` as a synonym for convergence.

- **Arbitration idle**: the inertia slot reads `IDLE`; no logical convergence
  holder owns the slot.
- **Pending obligation**: `recheck_committed != recheck_settled`.
- **Semantic quiescence**: arbitration is idle and no committed recheck
  obligation remains.
- **Ready observation**: one successful `FiberHandle::ready()` invocation can be
  linearized at a semantic-quiescence observation inside that invocation.

The abstract predicate is:

```text
Q := slot == IDLE && committed == settled
```

`Q` is a semantic predicate, not yet a claim that separately loading those
fields is automatically one coherent snapshot. The production read/recheck
sequence remains part of what the model must validate.

An off-runtime Service mutation may legitimately leave:

```text
slot == IDLE && committed != settled
```

because the revision is durable truth while executor-dependent driving is best
effort. Therefore `IDLE -> quiescent` is not an invariant.

## Current protocol facts

The arbitration word has three legal states:

```text
IDLE      = no logical driver owns convergence
ACTIVE    = one holder owns convergence authority
RELEASING = that holder is testing whether it can release authority
```

Service visibility mutation first commits a durable recheck revision and only
then attempts to drive convergence. `kick()` behaves conceptually as:

```text
ACTIVE:
    an existing holder retains responsibility

RELEASING:
    attempt RELEASING -> ACTIVE
    if this wins, the existing holder remains the driver and must recheck

IDLE:
    if an executor is available, attempt IDLE -> ACTIVE and dispatch one driver
    otherwise leave the committed obligation outstanding
```

A holder exits through `RELEASING`, never by publishing `IDLE` before its final
recheck. A successful `RELEASING -> IDLE` CAS releases arbitration authority.
A failed CAS observing `ACTIVE` means a racing kick asked the same holder to
continue; it does not authorize a second driver.

The production implementation deliberately panics on impossible arbitration
word values. Framework invariant panics from detached convergence work are
reported and then resumed; no synthetic `IDLE` recovery is allowed.

## Production transition mapping at Phase 1 start

Source inspection for #99/#101 established the concrete Service mutation order
that the model must represent. ADR 0031's semantic commit is now implemented as:

```text
Active late provide:
    ServiceStore lock
    -> discover complete affected set
    -> commit each dependent recheck revision
    -> install the newly visible occurrence
    -> release ServiceStore lock
    -> kick affected Fibers

Visible remove:
    ServiceStore lock
    -> validate exact occurrence / visibility
    -> discover complete affected set
    -> commit each dependent recheck revision
    -> remove the visible occurrence
    -> release ServiceStore lock
    -> kick affected Fibers

Fiber Active-boundary transition:
    ServiceStore lock
    -> snapshot owned publication slots
    -> discover complete affected set
    -> commit each dependent recheck revision
    -> publish the provider FiberState transition
    -> release ServiceStore lock
    -> kick affected Fibers
```

`SemanticTarget` publication assignments are reconstructed through
`ServiceStore::occurrence_id`, which takes the same ServiceStore lock. Therefore a
target read sees either the pre-mutation visibility state or a post-mutation state
whose pre-existing dependents already carry the matching durable recheck. A Fiber
whose dependency projection registers after the affected-set snapshot cannot
complete its initial target read through the locked ServiceStore until the
mutation is visible, so it follows the other half of ADR 0031's race law.

The semantic-commit seam may nest only bookkeeping synchronization in the proved
direction `ServiceStore -> DependencyIndex/Registry`. Fallback Registry snapshots
retain every cloned Fiber until the ServiceStore lock is released so filtering
cannot run a potentially last-reference Fiber/plugin/config destructor inside the
critical section (ADR 0029). Settlement kicks remain outside all Service locks.

`FiberHandle::ready()` currently maps to the observer side as:

```text
if committed != settled:
    kick
wait for arbitration IDLE
observe FiberState
return only if IDLE && committed == settled
otherwise retry
```

The model must validate the interleavings around those separate observations; the
mapping above is not itself a claim that they form one atomic snapshot.

## Phase 1 questions

Phase 1 is complete only when the model and production tests can answer these
three questions with explicit evidence:

1. **Who owns the driver?**
   At most one actor may exercise convergence authority at a time, and only an
   actor that acquired that authority may perform holder-only transitions.
2. **Which revision did a recheck cover?**
   Advancing `recheck_settled` must be justified by an inspection that covers
   the acknowledged committed revision and its corresponding semantic state.
3. **Why may `ready()` return?**
   Every successful ready observation must have a valid semantic-quiescence
   linearization point inside that invocation.

These are stronger than merely proving the atomic word stays in `{0,1,2}`.

## Phase 1 named invariants

### LC-01 — Legal arbitration states

Healthy protocol execution never creates an inertia word outside
`IDLE | ACTIVE | RELEASING`.

Class: safety. This is a useful sanity property, not the main proof target.

### LC-02 — Unique convergence authority

At most one logical holder may exercise convergence authority. A task that has
been dispatched but has not started polling is distinct from logical ownership;
likewise an old task may still execute non-authoritative tail work after release.
The oracle must track authority, not merely count live futures.

Class: safety. Phase 1 core.

### LC-03 — Revision acknowledgement has inspection coverage

`recheck_settled` may advance to revision `r` only when the holder has performed
a valid recheck whose observed semantic data covers `r`. A newly committed
revision must not be acknowledged using an inspection of older target state.

Class: safety. Phase 1 core.

### LC-04 — IDLE releases authority, not obligations

`slot == IDLE` implies no holder retains convergence authority. It does **not**
imply `committed == settled`; an outstanding revision may remain while no
executor is available or between commit and kick.

Class: safety/specification correction. Phase 1 core.

### LC-05 — Racing kick preserves responsibility

A kick racing a holder's release must neither erase an outstanding obligation
nor create a second holder. If the kick wins `RELEASING -> ACTIVE`, the existing
holder remains responsible and its attempted release must not silently complete.
If the holder reaches `IDLE` before the kick, the obligation must remain visible
and a later legitimate driver path must be able to claim it.

Class: safety. Phase 1 core.

### LC-06 — Ready has a valid quiescence observation

For every successful `ready()` invocation there exists a point within the
invocation at which the production contract is entitled to observe semantic
quiescence. The property does not promise that no mutation may commit after that
linearization point.

Class: safety/history property. Phase 1 should model the non-blocking decision;
full Notify waiting belongs to Phase 2.

### LC-07 — Conditional eventual convergence

After a finite set of mutations, if a legitimate driver path is eventually
invoked, holders continue to be scheduled, awaited plugin/framework operations
complete, no runtime shutdown/abort occurs, and no framework invariant panic
occurs, outstanding obligations can eventually be acknowledged and arbitration
can return to semantic quiescence.

Class: liveness under explicit progress assumptions. Do not claim this from a
bounded Loom run alone. Phase 1 may only check finite drain scenarios.

## Model shape

Use a layered approach rather than either extreme of a fully independent toy
model or a runtime-wide synchronization abstraction.

### Layer A — semantic oracle / reference history

Track semantic facts that do not themselves synchronize production actors:

- mutation/revision commits,
- target version observed by each recheck,
- authority acquisition/release,
- dispatch/start state,
- ready invocation/observation/return,
- invariant failures.

The oracle must not accidentally serialize the implementation under test. Avoid
a global observation mutex on the critical path if that would add happens-before
edges absent from production.

### Layer B — smallest shared production arbitration seam

Prototype sharing the actual load/store/CAS/retry ordering for the private
arbitration/revision core with Loom synchronization types. Do **not** introduce a
workspace-wide synchronization trait, genericize `Fiber`/Plugin APIs, or model
all of Tokio merely to satisfy the tool.

The PoC must decide whether this seam remains smaller and clearer than an
independent model. If sharing production code requires large generic/runtime
abstractions, stop and record that result rather than forcing the design.

### Layer C — real Tokio contracts

Keep and extend real-runtime tests for integration facts outside the small model:
actual `tokio::sync::Notify`, task dispatch/polling, cancellation, plugin apply,
shutdown, and cross-module lifecycle behavior.

Shuttle may be added after the small model if its Tokio wrappers materially help
exercise longer async histories. Passing Shuttle schedules is not evidence for
weak-memory ordering correctness by itself.

## Phase 1 scenarios

Start small enough that at least a core subset can finish its declared
exploration range rather than merely exhausting a time budget.

1. Two kickers compete from `IDLE`.
2. One holder releases while one mutator commits and kicks.
3. One holder, one mutator, one ready observer.
4. Two mutators commit revisions around one holder's inspection/acknowledgement.
5. Commit while no executor is available, followed by a later legitimate drive.

Model publication, revision commit, kick, inspection, acknowledgement, release,
and observer reads as distinct scheduling points. Do not collapse
`publish+commit+kick` or `inspect+acknowledge+release` into one atomic model step.

The minimal model needs a small semantic payload/version in addition to the
arbitration word and revisions; otherwise LC-03 cannot distinguish "checked the
new revision" from "read an old target and later saw a new counter".

## Negative controls

A model that cannot detect a known-bad protocol is not accepted as correctness
evidence. Phase 1 must include deterministic negative controls, at least:

- restore the historical `ACTIVE -> IDLE -> recheck -> ACTIVE` release window;
- allow revision acknowledgement before the inspection covering that revision;
- allow a racing `RELEASING` kick to create a second logical holder.

Each mutation must cause the corresponding model invariant to fail. Where a
counterexample can be expressed through the public/runtime API, preserve it as a
real Tokio regression test as well.

## Phase 2 — ready/Notify protocol

After Phase 1 arbitration/revision work is credible, model the waiting layer:

- waiter registration versus notification,
- future creation/enable/check/poll ordering,
- multiple waiters,
- wake then recheck after a new mutation,
- waiter cancellation,
- exact semantics of `notify_waiters()` versus any future use of `notify_one()`.

Do not assume deleting `Notified::enable()` must fail unless the actual Cordis
protocol depends on a behavior for which `enable()` is authoritative.

Real Tokio tests should use controlled barriers/manual polling where possible;
timeouts are guards against hung tests, not the primary correctness proof.

## Phase 3 — Era replacement model

Model Era arbitration independently first, then add a small number of
cross-protocol tests.

Core Era properties:

- a live source grants at most one successful replacement claim;
- losing swaps return `Closed` before allocating a successor;
- one successful claim creates at most one successor candidate under the current
  no-retry contract;
- source disposal completes before successor handoff;
- closing/dead sources cannot authorize a new swap, while an already-valid claim
  may continue using the recipe it acquired before closure;
- unpublished failed candidates retain exactly one cleanup responsibility and
  cannot remain resident at the operation's promised failure boundary;
- caller cancellation cannot reclaim an already published successor or orphan a
  committed cleanup obligation.

Later combination scenarios should include Era competing with unfinished
convergence and old-Fiber ready waiters around successor handoff.

## CI policy target

The intended eventual shape is:

- **PR required**: small Loom models whose declared exploration range completes,
  historical negative controls, and real Tokio contract regressions;
- **scheduled/deeper**: larger actor counts, higher preemption/exploration bounds,
  Era failure/cancellation combinations, and optional Shuttle random/PCT runs;
- **replay**: every discovered counterexample gets deterministic replay material
  plus a stable regression where feasible.

CI output must distinguish "declared range fully explored" from "budget ended
without finding a failure". Tool dependency compatibility with the Rust 1.88
MSRV gate must be verified, not assumed.

## Phase 1 completion criteria

Phase 1 is not complete because a `loom` dependency or tests exist. It is
complete when all of the following are true:

- [ ] The production/spec terminology distinguishes arbitration idle from
      semantic quiescence.
- [ ] LC-01 through LC-06 have explicit executable evidence or an explicit
      documented reason a property remains outside the model.
- [ ] The model includes revision-to-inspection coverage, not only revision
      counters.
- [ ] The model includes a ready observer capable of detecting stale quiescence
      decisions.
- [ ] At least the smallest core scenarios complete their declared Loom
      exploration range in PR CI.
- [ ] The historical release-window negative control fails under the model.
- [ ] Premature revision acknowledgement fails under the model.
- [ ] Duplicate-holder mutation fails under the model.
- [ ] Production/model transition mapping is documented and reviewable.
- [ ] Real Tokio tests remain the authority for behaviors the model does not
      execute.
- [ ] Known coverage gaps and progress assumptions are documented.

## Non-goals

Phase 1 does not:

- claim formal verification of the Rust memory model;
- prove unbounded liveness;
- replace real Tokio tests;
- recover from framework invariant panics;
- model arbitrary plugin user code;
- genericize the whole runtime for a testing tool;
- include benchmark work, 1.0 readiness planning, or legacy `cordis-cli` work.

Those can proceed separately after the concurrency evidence has a credible core.

## Work log

### 2026-09-15

- Issue #96/#97 established report-then-resume-unwind behavior for detached
  framework invariant panics.
- External review corrected the earlier `IDLE == quiescent` shorthand and
  identified revision-to-semantic-inspection coverage as a central invariant.
- Chosen direction: Phase 1 Loom PoC around a minimal arbitration/revision core,
  with negative controls and real Tokio contracts retained. Do not commit to a
  large production synchronization abstraction before the PoC demonstrates that
  the seam is worthwhile.
- Source inspection found a concrete ADR 0031 mismatch before Loom was added:
  `Fiber::transition()` publishes the provider `FiberState` first, while Service
  visibility is derived directly from that state, and only afterwards calls
  `notify_dependents_for_edges()` to commit dependent recheck revisions. ADR 0031
  requires visibility mutation and durable affected-set recheck to be one semantic
  commit. This publication-to-revision window is now the first Phase 1 bug (#101) to pin
  with a deterministic regression before changing the protocol.
- Initial `cargo add loom@0.7.2` was blocked by crates.io connectivity; no Cargo
  files were modified. Tool installation must not block fixing a protocol defect
  already established from production code plus accepted ADR authority.
- Focused issue #101 replaced malformed #100 and repairs all three effective
  visibility-mutation paths (late provide, visible remove, Active-boundary
  transition) with revision-before-visibility ordering inside ServiceStore and
  outside-lock kicks.
- A deterministic late-provide regression pauses after the semantic commit but
  before the ordinary kick. `ready()` must still drive the committed obligation
  to `Active`. A temporary mutation that removed the durable commit made this
  exact test fail with `Pending`, confirming the regression discriminates the
  historical bug rather than merely exercising the fixed code.
