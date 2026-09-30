# Consumer guide

This is practical, non-normative guidance for composing the supported Cordis v3
API. The [glossary](../CONTEXT.md), [architecture](v3-architecture.md),
[public interface](v3-public-interface.md) and accepted ADRs own the contract;
the [compatibility policy](compatibility-policy.md) owns package promises.
Follow those authorities if a summary here is ambiguous. These ten rules explain
consumer choices; they do not add stability promises. For the reviewed baseline,
see the [conformance evidence](api-freeze-evidence.md),
[public naming review](api-naming-review.md) and
[freeze candidate recommendation](api-freeze-recommendation.md).

## 1. Prepare input, seal it, then spawn and retain the handle

Implement `Plugin` directly. Adapt source `Config` into typed `Input` with
`prepare`, seal that value with `PreparedPlugin::from_input`, then await
`Context::spawn`. Preparation is synchronous and precedes lifecycle admission;
a preparation failure or direct panic cannot leave an admitted Fiber. Sealing
proves the Plugin/Input type association, not which particular Plugin object
produced the input. Use concrete Plugin error types; `BoxError` is useful at an
outer application boundary such as `main`, not as the canonical associated error.

Keep each apply poll non-blocking. Move synchronous blocking sections to
`tokio::task::spawn_blocking` and await the returned JoinHandle asynchronously.
Do not rely on spawning-runtime affinity: apply may run on a Cordis-owned
runtime, and `tokio::spawn` and `Handle::current()` inside apply target the
runtime polling it. A blocked poll can stall unrelated Fibers' async cleanup and
lifecycle completion. Current placement is described, not promised, in
[ADR 0029's completion executor posture](adr/0029-lifecycle-commits-complete-and-critical-sections-are-closed.md#consequences-of-the-rule).

Successful spawn hands off a live, quiescent `FiberHandle`: Active means apply
succeeded; Pending means a required Service is unavailable and apply has not
run. Keep the delivered handle for explicit lifecycle control. Dropping it does
not dispose the Fiber. If the spawn future is dropped after its commit, core owns
the undelivered Fiber's cleanup; a delivered handle belongs in consumer composition.
Creation up to handle delivery is driven by the spawn future: one you keep but
stop polling, such as the unfinished half of a `select`, holds its new Fiber in
creation until you poll it again or drop it.

Authority: [preparation and creation](v3-public-interface.md#plugin-preparation-sealing-and-creation),
[ADR 0038](adr/0038-plugin-input-names-role-prepared-wrappers-name-stage.md),
[ADR 0039](adr/0039-consumer-fiber-control-is-a-fiber-handle.md).
Run `cargo run --locked -p hello_plugin`; its [source](../examples/hello_plugin/src/main.rs)
prepares EchoInput, spawns, records the handle in a Roster, dispatches Ping and
explicitly tears down.

## 2. Choose Service placement, Event reachability and configuration independently

Use `new_service_realm` and `with_service_realms` to share exact Service slots,
or `with_isolated_service` for a fresh private slot. A realm belongs to one
Runtime; equal textual Service names do not join different realms, and lookup
never falls back to an ancestor or default realm. A batch mapping validates
duplicates before foreign realms and installs nothing on failure.

Use `with_child_scope` for Event reachability and `with_intercept::<S>` for
already-prepared Service configuration layers. Neither implies resource
ownership, tenant authorization or Service placement. A Context clone preserves
all axes and current Fiber attribution; `root()` resets those axes and attribution
to the root of the same Runtime. It does not create another Runtime.

Authority: [Context axes](v3-public-interface.md#context-and-its-axes),
[ADR 0032](adr/0032-context-axes-are-orthogonal.md).
Run `cargo run --locked -p scopes_tenants`; its
[source](../examples/scopes_tenants/src/main.rs) checks private/shared Service
placement and ancestor/sibling Event routing independently. It does not implement
an authorization policy or demonstrate intercept composition.

## 3. Declare prerequisites and control the exact publication you created

Build `InjectSpec` before sealing. `require` and `require_configured` upsert one
effective requirement per Service; a later `require` clears its configured layer.
For a configurable Service, call its `prepare_config` before installing a Layer.
`resolve_config` supplies base, outer-to-inner Context layers and head to the
Service's `compose_config`; the Service defines their composition, with no
framework-wide merge or default. Config, Layer and Resolved are distinct roles.

Publish with `provide`, look up with `try_service`, and retain the returned
`ServicePublication` if you need exact mutation. `set` replaces the payload of
that occurrence without changing the dependency target; `remove` withdraws it.
Dropping the publication capability leaves generation cleanup armed. Loading
publications occupy slots but become visible only when their provider is Active.
Direct lookup needs no InjectSpec membership; declarations govern lifecycle
prerequisites. After visibility changes, `ready` drives current-target settlement,
including durable drift committed outside an executor; stable Pending remains a
valid quiescent result.

Authority: [Service configuration](v3-public-interface.md#dependency-declarations-and-service-configuration),
[publication and lookup](v3-public-interface.md#service-publication-and-lookup),
[ADR 0031](adr/0031-service-convergence-tracks-exact-publication-assignments.md).
Run `cargo run --locked -p scopes_tenants`; its
[source](../examples/scopes_tenants/src/main.rs) publishes and performs exact
lookups. Configuration, set/remove and convergence boundaries are demonstrated
by the [Service contracts](../crates/cordis-core/tests/service_v3.rs), rather than
claimed as behavior exercised by that tour.

## 4. Choose same-Fiber control or replace the era, then retain the right handle

Use `ready` to drive the latest target to quiescence, including Pending;
`wait_state` passively awaits a state publication with a timeout. Restart retries
the same Fiber and input. For new input, prepare and seal one
`PreparedChange::from_input::<P>`, then consume it through `update` or `era_swap`.
Every candidate permits one attempt, including mismatch, veto and failure.

Handle `WaitStateError::Elapsed`, `Recursion` and `DeadlineUnavailable` separately.
The last means the process refused to start the shared deadline thread, not that
the timeout elapsed. Existing matches on this non-exhaustive enum still need a
wildcard. Satisfied waits and unrepresentable monotonic deadlines need no thread.

`on_update` installs typed precommit Mapper/Around policy. Tail acceptance is
provisional until outer callbacks return and admission is revalidated. Veto,
control failure and admission refusal do not commit the candidate. After update
commit, new input remains authoritative even if apply fails; use the
operation-specific error to decide whether to call `ready` or restart. A
successful `Committed` may carry Pending. Update preserves FiberId.

Era swap uses the captured immutable spawn recipe, skips update policy, ends the
old Fiber before creating a fresh successor and awaits dependent convergence.
On success, replace your retained control with the returned handle. `Incomplete`
means the old Fiber is gone and attempted-successor cleanup and final convergence
finished; it does not offer old-era rollback. Postcommit caller cancellation
cannot strand framework completion, but an undelivered successor is cleaned up.

Authority: [lifecycle](v3-public-interface.md#fiber-identity-lifecycle-and-typed-group-removal),
[typed control](v3-public-interface.md#typed-update-control),
[ADR 0030](adr/0030-era-replacement-ends-one-fiber-before-creating-another.md),
[ADR 0034](adr/0034-update-control-is-precommit-and-non-dispatchable.md).
Run `cargo run --locked -p chat_capstone`; its
[source](../examples/chat_capstone/src/main.rs) asserts same-ID update, fresh-ID
era replacement and dependent convergence, then explicitly disposes the successor.
See [application teardown](application-teardown.md) for delivered-handle ownership.

## 5. Register resources with their generation and separate ownership from attribution

Register cleanup with `effect`/`effect_sync`, and generation-owned tasks with
`run`, using the intended apply Context. Admission either refuses without residue
or commits an owned occurrence; use from another Fiber does not transfer ownership.
Manual `EffectRegistration::dispose` competes with drain for one exact claim and,
after winning, completes independently of the caller. `disarm` suppresses that
cleanup. Registration Drop is inert. Drain closes admission and executes remaining
obligations sequentially in reverse commit order, continuing after error or panic.
Use synchronous cleanup for short bookkeeping; move blocking work into async
cleanup with `tokio::task::spawn_blocking`.

A `run` task binds to the runtime polling apply. When apply is framework-driven
(restart, update, background convergence or an era successor), that is currently
Cordis's completion runtime, so the task and its timers and IO live there. This
includes a Plugin that spawns Pending and is first applied once its Service is
published. To keep a long-lived workload on your application runtime, carry that
runtime's `tokio::runtime::Handle` in `Input` and have the `run` task await work
spawned through it. That work must still end when the generation's cleanup
closes its inputs, because drain joins the `run` task cooperatively.

`spawn_attributed` returns a user-owned Tokio JoinHandle and carries live settle
attribution for recursion checks; it adds no generation cleanup or join ownership.
Inherited attribution expires with its source settle scope. Framework-owned async
completion survives loss of the origin executor while the process remains alive;
it cannot extend the lifetime of an external driver captured earlier or drain at
process exit. Root registrations require their own explicit controls.

Keep the `Drop` of every value you hand to Cordis panic-free: Plugin inputs,
Service values, closures and their captures, Event values and exporters. Cordis
still drops them outside its locks, but it contains a destructor panic only on a
best-effort basis and promises no specific outcome; see the
[user destructor rule](v3-public-interface.md#user-destructor-panics) and
[ADR 0041](adr/0041-user-destructor-panics-are-best-effort.md).

Authority: [effects and tasks](v3-public-interface.md#effects-and-tasks),
[ADR 0028](adr/0028-fiber-generations-own-cleanup-runtime-owns-residency.md),
[ADR 0029](adr/0029-lifecycle-commits-complete-and-critical-sections-are-closed.md).
Run `cargo run --locked -p worker_daemon`; its
[source](../examples/worker_daemon/src/main.rs) registers cleanup and a `run` task
that owns interval polling. Attribution expiry and manual-claim cancellation are
covered by the [evidence](api-freeze-evidence.md#generation-ownership-and-consumer-teardown),
not asserted as behaviors exercised by that example.

## 6. End delivered Fibers with an explicit application teardown policy

Retain every delivered handle whose Fiber your application intends to end.
Call `dispose().await` for a terminal barrier through cleanup, Disposed publication
and exact residency unlink. Once committed, `dispose()` and `remove_plugins`
finish even if their futures are dropped or no longer polled. Await them; do not
block a Tokio worker thread on a Cordis lifecycle future (for example with
`futures::executor::block_on` in a task or a `Drop`), because the committed work
may be queued on that same worker. Use `block_in_place` or `spawn_blocking` from
synchronous code. Dropping Context, FiberHandle or LoadOutcome does not end a
Fiber. Spawn origin is provenance only: a spawned Fiber can outlive its spawn
origin, and nothing cascades. There is no Runtime-wide shutdown operation.

Compose ordering in the application. The examples' Roster records delivered
handles in spawn order, disposes in reverse order and attempts every handle even
when one disposal is refused. This is a demonstrated consumer policy.
`remove_plugins::<P>` separately selects one current typed allocation, detaches
it atomically and drains its frozen members; later same-type spawns are outside
that removal, absence succeeds, and no inter-Fiber disposal order is promised.

Authority: [application teardown](application-teardown.md),
[lifecycle/removal](v3-public-interface.md#fiber-identity-lifecycle-and-typed-group-removal).
Run `cargo run --locked -p worker_daemon`; its
[source](../examples/worker_daemon/src/main.rs) and shared
[Roster](../examples/common/src/lib.rs) demonstrate explicit teardown.
The [Roster tests](../examples/common/tests/boot.rs) separately verify reverse
order, attempt-all after refusal and repeat-call idempotence.

## 7. Select an Event role and routing, then await the intended completion

Declare `Event::{NAME, Args, Output}`, adapt callbacks as Observer, Responder,
Mapper or Around, then register with `on`/`on_with`. Sync adapters complete
immediately and can still fail. `with_state` creates invocation-local state after
preflight and a successful claim; it does not create persistent shared state.
Callbacks receive their registration Context. Supply `Routing` explicitly:
Scoped uses Event reachability; Unscoped selects across scopes in this Runtime.

Choose ordered fail-first `emit`, complete claim/start-and-await-all
`emit_parallel`, sequential first-answer `query`, or owned Mapper/Around
`waterfall`. QueryOutcome distinguishes Answer from Miss even for false/zero/empty
answers. `waterfall_query` derives a query tail with the same routing. Around's
`Next` is consuming and single-use. A once occurrence is consumed before callback
invocation, even if it fails or the caller cancels afterward. Exact removal cannot
revoke already claimed work. Await the operation if callback completion matters:
Event futures remain caller-owned and pending-callback cancellation does not
promise detached completion or rollback of claims.

Authority: [Events](v3-public-interface.md#event-contracts-roles-and-dispatch),
[errors](v3-public-interface.md#event-errors),
[ADR 0033](adr/0033-events-are-typed-completion-aware-and-occurrence-claimed.md).
Run `cargo run --locked -p gateway` for
[scoped waterfall/query](../examples/gateway/src/main.rs), or
`cargo run --locked -p chat_capstone` for
[roles, registration Context and invocation-local state](../examples/chat_capstone/src/main.rs).
The [evidence](api-freeze-evidence.md#event-dispatch-and-exact-claims) covers
cancellation and failure correlation separately; destructor panics follow the
best-effort rule in rule 5.

## 8. Observe committed facts and install explicit Logger exporters

Use `observe_runtime` with an Observer adapter for best-effort postcommit
Runtime records. Delivery is detached, parallel and attempt-all; observer failure
or a missing executor cannot veto the source operation. Ownership and callback
Context come from the registering generation. Recover current facts with
`runtime_snapshot` after a delivery gap. Its records are individually consistent;
the combined collections are neither a globally linearizable instant nor ordered
or referentially closed. Correlation IDs grant no control, replay or audit history.

Use a named `Logger` channel and explicitly install exporters. Filtering uses
`min_level(channel).unwrap_or(default_level())`; None means fallback, not disabled.
Panic-contained exporter attempts run outside locks. Exact removal affects future
snapshots but cannot revoke an in-flight one; registration Drop is inert.
`LogRecord::sequence` orders Runtime-local assignment, not callback receipt or
persistence. `BufferExporter` is an opt-in bounded adapter in receipt order,
with nonzero-capacity construction, snapshot and clear.

Authority: [observation](v3-public-interface.md#runtime-snapshots-and-observations),
[Logger](v3-public-interface.md#logger),
[ADR 0035](adr/0035-runtime-observation-follows-protocol-truth.md).
Run `cargo run --locked -p logging_exporters`; its
[source](../examples/logging_exporters/src/main.rs) checks filtering, bounded
buffering, exact removal, record accessors, snapshot correlation and source success
despite a failing observer. It does not demonstrate a reliable observation journal.

## 9. Freeze a Loader plan, inspect every outcome and retain delivered handles

Build with `LoadPlanBuilder`, using only already-admitted same-lineage parent
EntryIds, then `finish`. Structural parents order execution; they do not own
Fibers. Config is required in the JSON source schema (unit is null). Resolve keys
prefer key over name and may repeat. Correlate by EntryId, which survives plan
clones/reuse; each execution creates fresh lifecycle and realm occurrences.
Private/Shared source policy allocates opaque Runtime-local Service realms;
text labels never rendezvous across executions or enter core.

Parse source rows strictly: every source-schema object and tagged variant
rejects unknown or misplaced fields, so a misspelled `disable` (for `disabled`)
now fails instead of being silently ignored.
Remove unsupported fields before loading; this intentional pre-1.0 Deserialize
change is recorded in the [Loader changelog](../crates/cordis-loader/CHANGELOG.md).
Valid wire forms and Serialize output are unchanged.

A synchronous `PluginResolver` recognizes a request, prepares typed Plugin and
configured-Service inputs and returns sealed PreparedPlugin, None for unknown
key, or a typed error. Direct JSON helpers retain preparation errors and allow
ordinary pre-lifecycle panic unwind; the Loader resolver boundary normalizes
returned errors and panics once. Resolver requests have no Context or placement
control. Loader then performs the complete core spawn.

Inspect all ordered entries: disabled entries prune descendants, ordinary failure
does not. Load is partial; `is_ok` means no Failed entry, not all Fibers Active.
Use `fiber_handles` to retain delivered controls in your application roster.
Before final handoff, dropping the load future causes reverse-success-order
attempt-all framework rollback; a load future kept but no longer polled holds its
in-progress spawn, as a spawn future does, and rolls nothing back. Ordinary row
failure keeps earlier successes caller-owned.
After delivery, dropping LoadOutcome is inert. Teardown remains explicit.

Authority: [plan/source](v3-public-interface.md#loader-plan-and-source-schema),
[resolver/outcomes](v3-public-interface.md#loader-resolver-execution-and-outcomes),
[ADR 0036](adr/0036-module-and-crate-seams-are-semantic.md),
[application teardown](application-teardown.md).
Run `cargo run --locked -p gateway`; its
[source](../examples/gateway/src/main.rs) boots a frozen plan from
the embedded `GATEWAY_JSON`, adapts typed inputs and reports deliberate
failed rows alongside successful handles before explicit teardown.

## 10. Check Timer registration, then distinguish elapsed from cancellation

Import `TimerExt` from `cordis_timer` and construct `sleep`, `timeout` or `interval`
in a usable time environment. Construction is synchronously fallible and atomic:
check local arguments, Context admission and the time environment/deadline before
accepting the delivered operation. Zero one-shot delay is valid; interval period
must be nonzero. A later generation close produces TimerCancelled, not a
registration failure. Deadlines and interval phase are pinned at construction.

Timeout work is lazy and caller-owned, with no universal Send/static requirement.
Generation cleanup signals timer cancellation; it never polls, moves or drops
that work. Elapsed is a normal timeout result, distinct from cancellation.
Completed can commit only if the deadline is still unelapsed after work reports
Ready. Work polling panic follows ordinary caller unwind. Dropping abandons the
operation/work and disarms cleanup where possible.

Poll one-shots through exactly one terminal result; repolling afterward panics.
An interval starts at anchor + period, coalesces late polls to one overdue tick
without catch-up bursts or phase shift, and emits one cancellation error followed
by None forever. Dropping it emits nothing.

Authority: [Timer](v3-public-interface.md#timer-facade-and-operations),
[ADR 0036](adr/0036-module-and-crate-seams-are-semantic.md).
Run `cargo run --locked -p worker_daemon` for
[generation-owned sleep/interval use](../examples/worker_daemon/src/main.rs), or
`cargo run --locked -p gateway` for
[completed/elapsed/cancelled timeout outcomes](../examples/gateway/src/main.rs).
The deterministic hard deadline race is documented in the
[evidence](api-freeze-evidence.md#timer-construction-arbitration-and-terminal-ownership).
