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

**Phase-1 decision:** do not extract a production-shared synchronization seam.
The production protocol currently composes three `std` atomics with
`parking_lot` target storage, ServiceStore-backed target reconstruction,
`tokio::sync::Notify`, runtime discovery, and task dispatch across
`commit_recheck`, `exit_recheck`, `kick`, and `FiberHandle::ready`. Sharing the
actual atomic operations with Loom would require a test-oriented atomic/sync
abstraction or alternate feature build while still leaving the ServiceStore and
Tokio boundaries outside that shared core. That is more design distortion than
evidence gained at this stage. Phase 1 therefore keeps reduced models with exact
production-transition mapping, discriminating historical negative controls, and
real Tokio regressions for integration behavior. Revisit this decision if the
production atomic protocol changes materially or model/production drift becomes
a demonstrated maintenance problem.

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

Current evidence maps to all five scenarios: the two-IDLE-kicker CAS model covers
(1); LC-05 release/kick authority covers (2); LC-06 ready history covers (3);
the bounded two-mutator inspection/acknowledgement model covers (4); and the
off-runtime durable-obligation finite-drain model covers (5). The larger
four-actor scenario (4) declares `preemption_bound = 2` and `max_branches = 48`;
it is bounded coverage, not an unbounded/exhaustive claim. The smaller LC-06
history model declares `max_threads = 3` and `max_branches = 64` with no
permutation or duration cap.

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
- allow a racing `RELEASING` kick to create a second logical holder;
- let `ready()` treat arbitration `IDLE` as semantic quiescence after a
  pre-invocation Service commit;
- replace the IDLE claim CAS with load-then-store so two kickers can both win;
- consume an off-runtime committed revision merely because no executor exists.

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

### Phase 2 signal-contract findings

Cordis currently produces only `notify_waiters()` on the inertia wake channel.
With the currently locked Tokio 1.53.1, the observable `notify_waiters`
contract makes future creation the broadcast observation boundary. Therefore the
current lost-wakeup boundary is **create `Notified` before re-checking the slot**:
a broadcast that lands after creation but before `enable()` or first poll is
still observed.
`Notified::enable()` is not required for that broadcast guarantee.

Real Tokio contract tests pin five distinct facts:

- a `notify_waiters()` between future creation and first poll completes that
  future;
- a `notify_waiters()` before future creation leaves no stored permit for that
  later future, pinning creation as the broadcast observation boundary;
- one `notify_waiters()` reaches every pre-created `Notified` future;
- cancelling one pre-created waiter does not consume another waiter's broadcast;
- when two futures are explicitly `enable()`d, one `notify_one()` permit selects
  exactly one waiter and a second permit releases the other.

The last test explains why retaining `enable()` is still useful stronger
choreography even though current Cordis producers broadcast: it eagerly enters
the single-permit waiter queue and keeps the consumer pattern safe if a future
producer deliberately changes to `notify_one()`. Do not describe `enable()` as
the current `notify_waiters()` lost-wakeup authority.

Loom's `sync::Notify` is not an adequate substitute for this contract: it is a
single-waiter park/unpark primitive and does not model Tokio `Notified` future
creation-time broadcast observation, `enable()`, or multi-waiter selection.
Phase 2 therefore treats these real Tokio tests as the authority for notification
semantics. A reduced Loom model may still cover Cordis's abstract state/recheck
ordering around a wake, but must not claim to verify Tokio `Notify` internals.

### Phase 2 retry/cancellation findings

Deterministic manual-poll tests now cover Cordis's retry layer above Tokio's
signal contract:

- a `ready()` waiter first blocks on a busy slot, receives an older IDLE wake,
  then sees an off-runtime Service mutation commit before its next poll; that
  stale wake is not accepted as quiescence, the durable recheck is driven, and
  the future waits again for convergence to the new target;
- cancelling that `ready()` caller after it has kicked convergence does not
  cancel framework-owned progress; a replacement `ready()` observes the final
  `Active` state with no outstanding recheck;
- when two `ready()` waiters are registered, cancelling one does not prevent the
  surviving waiter from observing the release;
- when two `claim()` waiters are registered, cancelling one does not consume the
  release; the surviving waiter alone claims the slot.

These tests deliberately use manual `Future::poll` boundaries instead of timing
to place cancellation and semantic mutation between exact protocol steps.
Timeouts are used only after framework-owned async convergence has been kicked,
as a hang guard for eventual task completion.

### Phase 2 completion criteria

Phase 2 is complete when all of the following are true:

- [x] `notify_waiters()` creation/poll ordering is pinned by real Tokio contract
      tests.
- [x] Multiple pre-created waiters observe one broadcast.
- [x] `notify_waiters()` and `notify_one()` semantics are explicitly
      distinguished, including `enable()`'s single-permit queue role.
- [x] A wake followed by a newer semantic mutation forces `ready()` to recheck
      and retry rather than accept the old wake as quiescence evidence.
- [x] Cancelling one `ready()` waiter does not block another waiter.
- [x] Cancelling one `claim()` waiter does not consume another claimant's
      release opportunity.
- [x] Cancelling a `ready()` caller after it drives convergence does not cancel
      framework-owned progress.
- [x] Tokio `Notify` internals are not falsely claimed as Loom-verified.
- [x] The final Phase-2 contract set passes the repository's required PR CI
      matrix on the merge candidate.

## Phase 3 — Era replacement model

Model Era arbitration independently first, then add a small number of
cross-protocol tests.

### Phase 3 invariants

- **ER-01 — unique live-source terminal claim.** A live source grants at most one
  successful replacement claim. Era swap and ordinary disposal share this same
  Open-to-Closing authority.
- **ER-02 — losers allocate nothing.** A swap that loses to another swap or to
  ordinary disposal returns `Closed` before attempting a successor.
- **ER-03 — one claim, one candidate.** One successful Era claim creates at most
  one successor candidate under the current no-retry contract.
- **ER-04 — death before birth.** The source's full terminal barrier completes
  before successor creation is attempted; therefore successor visibility and
  handoff also occur strictly after old-Fiber death.
- **ER-05 — closure cannot mint new authority.** Closing/dead sources cannot
  authorize a new swap, while an already-valid committed Era owner may continue
  using the recipe captured before it closed the source.
- **ER-06 — failed candidates keep one cleanup responsibility.** An unpublished
  failed candidate cannot remain resident at the operation's promised failure
  boundary and its terminal cleanup is owned exactly once.
- **ER-07 — cancellation transfers no ownership back to the caller.** Preclaim
  cancellation is no-effect; postclaim cancellation cannot orphan source death,
  candidate success-or-cleanup, final convergence, or an undelivered handoff.
- **ER-08 — handoff follows final current-target convergence.** Success and every
  incomplete result finish affected-dependent convergence before the operation's
  externally observable handoff/failure boundary.

### Phase 3 arbitration findings

The first reduced Loom layer deliberately treats the already-modeled lifecycle
slot as a mutex: Phase 1 proves unique slot authority and Phase 2 pins its Tokio
wait/wake choreography, so the Era model should not pretend to re-verify either.
The mutex covers only the production `disposing`-style terminal claim decision.
After that guard is released, successor-attempt accounting proceeds independently,
matching production's separation between source lifecycle ownership and later
candidate work. Death-before-birth ordering is modeled separately below.

The model now establishes:

- two racing swaps choose exactly one source owner and exactly one successor
  attempt;
- a swap racing ordinary disposal attempts a successor iff the swap owns the
  unique source terminal claim;
- a synthetic allocation-before-claim variant lets two racers allocate against
  one eventual source claim and is detected;
- a synthetic successor-retry variant creates two candidates from one source
  claim and is detected;
- publishing successor creation only after source terminal completion preserves
  death-before-birth; reversing that order exposes the forbidden middle state.

All six first-layer tests use `max_threads = 3` and `max_branches = 64`, with
no permutation or duration cap. They therefore complete the declared finite
range rather than treating a search budget ending as success.

This is ER-01 through ER-04 evidence only. It does not model candidate cleanup,
caller cancellation, final dependent convergence, or Tokio notification.
Existing real Tokio Era regressions remain the authority for those concrete
runtime behaviors until later Phase 3 slices map them explicitly.

### Phase 3 completion criteria

- [x] ER-01 unique source ownership is modeled for swap-vs-swap and
      swap-vs-dispose.
- [x] ER-02 loser-before-allocation is executable and has an allocation-before-
      claim negative control.
- [x] ER-03 no-retry candidate ownership is executable and has a retry negative
      control.
- [x] ER-04 death-before-birth ordering is executable and has an inverted-order
      negative control.
- [ ] ER-05 closing/dead-source admission and captured-recipe continuation have
      explicit evidence or a documented structural argument.
- [ ] ER-06 failed/unpublished candidate cleanup and no-residency are mapped to
      systematic evidence.
- [ ] ER-07 preclaim/postclaim cancellation ownership is mapped to systematic
      evidence.
- [ ] ER-08 final dependent convergence and handoff/failure ordering are mapped
      to systematic evidence.
- [ ] Cross-protocol Era/convergence scenarios cover unfinished convergence and
      old-Fiber `ready()` around successor handoff.
- [ ] The final Phase-3 contract set passes the repository's required PR CI
      matrix on the merge candidate.

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

- [x] The production/spec terminology distinguishes arbitration idle from
      semantic quiescence.
- [x] LC-01 through LC-06 have explicit executable evidence or an explicit
      documented structural argument. LC-01 is structural: production writes
      only the three named constants and panics on every other observed word;
      the executable models exercise the legal transition set.
- [x] The model includes revision-to-inspection coverage, not only revision
      counters.
- [x] The model includes a ready observer capable of detecting stale quiescence
      decisions.
- [x] At least the smallest core scenarios complete their declared Loom
      exploration range in PR CI; #109 completed that declared range across the
      repository's required CI matrix.
- [x] The historical release-window negative control fails under the model.
- [x] Premature revision acknowledgement fails under the model.
- [x] Duplicate-holder mutation fails under the model.
- [x] Production/model transition mapping is documented and reviewable.
- [x] Real Tokio tests remain the authority for behaviors the model does not
      execute.
- [x] Known coverage gaps and progress assumptions are documented.

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

- After #102 merged #101, the Phase 1 branch was rebased onto `main` and Loom
  0.7.2 was added as a dev-only workspace dependency. Cargo resolved a Rust
  1.88-compatible dependency set.
- The first reduced Loom suite now has six two-thread tests. Positive models cover
  revision-before-visibility semantic commit, revision-to-inspection coverage,
  and guarded `ACTIVE -> RELEASING -> IDLE` release. Negative controls restore
  the pre-#101 visibility-before-revision ordering, acknowledge a new revision
  without re-inspecting target state, and restore the historical
  `ACTIVE -> IDLE -> recheck` release window; Loom finds all three counterexamples.
- The guarded-release oracle originally used two independent booleans and produced
  a false positive when the recheck completed between the observer's reads. It
  was corrected to bracket the observation with one `NOT_STARTED / RECHECKING /
  COMPLETE` phase word. This is retained as a reminder that model assertions
  themselves require concurrency review.
- At the #104 checkpoint, the remaining gaps were a production-shared
  synchronization seam, LC-05 authority identity, LC-06 ready history,
  Notify/lost-wakeup behavior, multi-mutator histories, and Era. This was
  reference-model evidence, not formal verification.

- LC-05 now has a packed authority-state model for a kick overlapping release.
  Legal schedules end either with owner A reactivated by `RELEASING -> ACTIVE`
  or with owner B claiming only after A has published `IDLE`. A negative control
  that transfers `RELEASING` directly to B is detected as duplicate logical
  authority. This models authority identity rather than counting live futures.
- The #106 LC-05 change intentionally left LC-06 for a separate history model:
  a final-state snapshot would either reject legal overlap histories or prove
  its own instrumentation.

- LC-06 now has an invocation-history model for the non-blocking `ready()`
  decision. The oracle records only whether the single modeled Service mutation
  had reached its semantic publication point before invocation; it does not
  participate in protocol decisions. Returning the old state is accepted when
  publication races after invocation (a legal pre-mutation linearization point)
  and rejected when publication already preceded invocation. A negative control
  that treats arbitration `IDLE` as semantic quiescence reproduces the stale
  off-runtime commit-to-kick window. Full Notify/lost-wakeup behavior remains
  Phase 2.

- The LC-06 positive history model declares `max_threads = 3` and
  `max_branches = 64`, with no permutation or duration cap. Its state space is
  intentionally finite: one semantic mutation, one ready actor, and the main
  test thread. CI therefore exhausts that declared range instead of stopping on
  a time/permutation budget. The full Notify wait/retry protocol remains Phase 2.

- Phase-1 closeout adds the remaining scenario evidence. Two independent IDLE
  kickers have exactly one CAS winner; a load-then-store negative control lets
  both win and is detected. An off-runtime commit leaves `IDLE` with
  `committed != settled` and a later legitimate driver drains it; a negative
  control that consumes the revision without a driver leaves stale state and is
  detected.
- The two-mutator history uses four actors with `max_branches = 48` and
  `preemption_bound = 2`. Any intermediate acknowledgement is permitted only
  when the holder's inspected target covers the observed revision; after both
  mutations stop, one explicit finite drain reaches revision/target 2. The
  unbounded variant was intentionally rejected after its state space failed to
  complete within the local execution window.
- Layer B was evaluated and rejected for Phase 1: sharing actual atomic
  operations with Loom would require a test-oriented synchronization abstraction
  across code that is intentionally coupled to ServiceStore target reads,
  `parking_lot`, Tokio notification, runtime discovery, and task dispatch. The
  reduced-model + transition-map + Tokio-contract approach remains the smaller,
  clearer evidence architecture for the current protocol.
- #109 completed the declared Phase-1 model range in required PR CI on Rust
  1.88, latest stable, Linux, macOS, and Windows. With that external execution
  evidence recorded, every Phase-1 completion item is now satisfied; Notify
  waiting remains explicitly Phase 2 and Era replacement remains Phase 3.
- Phase 2 starts by pinning Tokio notification semantics rather than assuming
  `enable()` is the broadcast lost-wakeup boundary. Real Tokio tests prove that
  `notify_waiters()` is observed by every `Notified` future created before the
  broadcast, even before first poll; cancelled waiters do not consume another
  waiter's broadcast. A separate `notify_one()` test proves `enable()`'s actual
  single-permit queue role. Production keeps `enable()` as stronger choreography
  while comments now name future creation as the current broadcast boundary.

- Phase 2 retry/cancellation evidence manually polls `ready()` and `claim()` at
  exact waiter boundaries. An old IDLE wake followed by an off-runtime Service
  commit cannot return stale `Pending`; `ready()` drives the durable obligation,
  waits again, and framework-owned convergence survives caller cancellation.
  Separate two-waiter tests show cancellation of one `ready()` or `claim()`
  waiter cannot consume the survivor's release progress.
- #113 completed the final Phase-2 contract set in required PR CI on Rust 1.88,
  latest stable, Linux, macOS, and Windows. With that external execution
  evidence recorded, every Phase-2 completion item is now satisfied; Era
  replacement remains Phase 3.

- Phase 3 begins with a reduced Era arbitration model rather than reusing the
  convergence model. The production lifecycle slot is abstracted as one mutex
  because Phases 1/2 already carry authority/wakeup evidence; the Era layer keeps
  only terminal-source ownership plus successor-attempt state. Racing swaps and
  swap-vs-dispose admit exactly one terminal owner, losers allocate nothing, and
  the winner attempts one candidate under the no-retry contract. Three negative
  controls prove the model can detect allocation before claim, a second candidate
  attempt after one claim, and successor birth before complete old-Fiber death.
