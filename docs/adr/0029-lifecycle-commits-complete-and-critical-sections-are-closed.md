# Lifecycle commits complete independently and critical sections are closed

Status: accepted

Every deep lifecycle operation names exactly one irreversible commit.
Cancellation before that commit has no framework effect: no state,
configuration, gate, claim, allocation, publication, or observation
changes. Cancellation after that commit abandons only the caller's wait;
framework-owned work continues independently of caller polling until it
reaches the operation's documented barrier. Runtime-agnostic Cordis lifecycle
work normally stays on the current Tokio executor; if shutdown drops it after a
normal `Pending`, the same pinned future transfers to a shared Cordis completion
runtime. Arbitrary async effect cleanup is different: it is first polled on that
completion runtime, so Tokio time/IO work created by the cleanup never migrates
between runtime drivers. A poll unwind is a failure, never a transfer signal.
This uniform law covers
disposal, Registry removal, restart, update, era swap, new-Fiber
creation, exact manual cleanup disposal, and Loader result handoff.

One logical arbiter per Fiber serializes lifecycle intents: at most one
lifecycle operation advances on a Fiber at a time, accepted restart and
update intents serialize in admission order, provider drift coalesces to
the latest target, and terminal closing prevents later uncommitted
intents. The arbiter is logical ownership of the lifecycle transaction,
never an OS mutex held across user work or arbitrary awaits.

Framework critical sections protect bookkeeping only. User callbacks,
conversions, re-entrant clones, future polling, task spawn, awaits,
lifecycle work, notification, rollback, and potentially last-reference
destruction happen after synchronization is released: values are
extracted under lock and destroyed afterward, on success, refusal,
stale no-op, rollback, and error alike. No lock guard crosses an await
or poll boundary, and there is no global lock hierarchy — any nested
bookkeeping section is private to one deep seam, uses one proved
direction, calls no user or re-entrant work, and releases before
notification, destruction, rollback, or lifecycle work.

Gated publication is the one narrow proven atomic ownership-transfer
seam. Its only permitted nesting direction rechecks the generation gate
and then atomically commits the resource occurrence plus its matching
generation-owned cleanup, with every correctness-required durable
follow-up owned before the commit returns. Refusal publishes nothing and
leaves rollback with the resource owner. A publish-versus-close race has
exactly two outcomes: either the generation close wins and no resource
or cleanup control is delivered, or the commit wins and exactly one
generation-owned cleanup later removes or cancels the resource. There is
no third "visible but unowned" state.

## Rationale

This prevents stranded postcommit state, missed cleanup, reentrant
deadlock, and fresh-allocation removal without imposing a global lock
hierarchy. The named commit gives every operation a before/after law a
caller can reason about; independent completion guarantees that admitted
framework work reaches its barrier even after its caller leaves; closed
critical sections let user and re-entrant code complete instead of
deadlocking; and exact-allocation detach and prune make removal races
deterministic without ordering locks across core.

## Irreversible commits

| Operation | Irreversible commit | Required independent completion |
| --- | --- | --- |
| Fiber disposal | Open-to-Closing terminal claim | cleanup, Disposed, unlink/prune |
| Registry removal | detach current PluginGroup allocation | dispose every detached Fiber |
| restart | irreversible closure/replacement of the old generation | current-target quiescence |
| update | new config plus generation-replacement commit | committed-config target quiescence |
| era swap | source swap claim | old disposal, successor outcome/cleanup, final convergence |
| new-Fiber creation | allocation/publication requiring rollback responsibility | FiberHandle delivery or undelivered-Fiber disposal |

The same law governs the two remaining deep operations. A winning exact
manual cleanup dispose commits at its claim and completes under
framework ownership despite caller cancellation. Loader result handoff
commits as each FiberHandle is obtained; if load or result construction is
abandoned before delivery, the already-obtained FiberHandles are disposed in
reverse success order, attempt-all, under framework-owned completion. Loader
uses the same core completion seam, so abandoning a load during executor
shutdown does not drop that rollback. Dropping a delivered outcome is inert.

## Consequences of the rule

**Completion executor posture.** One lazily initialized process-wide Tokio
runtime with two worker threads is shared by executor-shutdown transfers and by
async effect cleanup. This is a fixed process-lifetime cost, not one OS
thread/runtime per pending task or shutdown transfer. Runtime-bound resources
created by async cleanup bind to this completion runtime. Resources captured
earlier from an external runtime remain owned by that runtime; Cordis completion
ownership cannot keep an unrelated runtime's timer/IO driver alive after it
shuts down.

**Generation gate closure.** A generation admits cleanup and resource
registrations only while its gate is open: Loading and Active admit,
while stable Pending, stable Failed, closing, and Disposed reject before
publication or task start and deliver no resource and no cleanup
control. The root Fiber's generation remains open for Runtime duration.
Gate close freezes new admissions while the obligations the generation
already owns drain.

**Exact manual cleanup claims.** A consuming manual dispose or disarm
and the automatic generation cleanup arbitrate one exact claim per
obligation. If manual control wins, dispose runs the cleanup once —
completing under framework ownership despite caller cancellation and
reporting a returned error or panic as an `EffectFailure` without
restoring the occurrence — or disarm releases the obligation without
running it. If generation cleanup wins, the consumed manual operation
reports `false` and does not repeat cleanup. Dropping the registration
is inert.

**Sequential LIFO attempt-all drain.** Generation close claims every
remaining obligation and runs them sequentially in strict reverse-commit
order — on unload, on restart or update replacement, on failed-apply
rollback, and on disposal. A failing or panicking cleanup is isolated to
its own item: the drain attempts every remaining entry, reports
diagnostics, and still reaches its lifecycle barrier.

**Reliable protocol signals.** Durable protocol state — not executor
availability or observer delivery — owns lifecycle truth, target drift,
operation completion, and waiter wakeups. A missing executor may skip
observations and kicks but cannot lose a committed change; the next
capable lifecycle driver converges it.

**Allocation-based recursion refusal.** A lifecycle self-wait from a
Fiber's own settle context is refused before any state, config, gate,
claim, or allocation effect, by attribution to the concrete Fiber
allocation rather than to any public id or state. The typed
`LifecycleRecursion` refusal names the operation and the FiberId and
covers ready, wait_state, restart, update, era swap, dispose, and typed
group removal — the last refused before any Registry detach. Raw
`tokio::spawn` does not inherit Tokio task-local attribution; user subtasks
that remain in the current settle dependency graph cross that task boundary
through `Context::spawn_attributed`. Transferred frames share source liveness
and stop refusing once the source settle scope ends, so detached subtasks cannot
retain stale recursion state. Legal unrelated-Fiber waits and the dynamic
era-swap backstops are preserved.

**Registry detach and exact-allocation prune.** Bulk removal commits at
detaching one current PluginGroup allocation. Attach-before-detach joins
the fixed removal set and is fully disposed; attach-after-detach creates
or joins a fresh current allocation outside that removal. After the
commit, framework-owned work disposes every frozen member through the
ordinary terminal barrier with no promised inter-Fiber order; absence is
success, later removals see only their then-current allocation, and
caller cancellation does not stop the drain. Final disposal releases the
exact residency occurrence, and conditional prune removes a mapping only
if it still names that exact empty allocation, so an old or detached
unlink can never delete a recreated group.

## Non-normative lineage

| V3 rule | Lineage classification | Historical evidence only |
| --- | --- | --- |
| Generation-scoped registration gate | supersedes part of an existing ADR | ADRs 0003 and 0015 admitted registrations more broadly |
| Cleanup ownership transfer and exact manual claim | collapses several historical ADRs into one final rule | ADRs 0004, 0010, and 0015 |
| Sequential LIFO completion and contained cleanup failure | preserves an existing ADR | ADRs 0003 and 0004 |
| Cooperative `Context::run` join | preserves an existing ADR | ADR 0004 |
| One lifecycle arbiter and forward-only committed transitions | collapses several historical ADRs into one final rule | ADRs 0003, 0013, and 0018 |
| Uniform commit-or-no-effect cancellation law | introduces a genuinely new decision | Earlier ADRs established individual pieces but no general law |
| Allocation-based lifecycle-recursion refusal | preserves an existing ADR | ADR 0019 |
| Closed critical sections plus gated publication | collapses several historical ADRs into one final rule | ADRs 0010 and 0015 |
| Detach-current-allocation removal and exact-allocation pruning | supersedes part of an existing ADR | ADR 0025 |
