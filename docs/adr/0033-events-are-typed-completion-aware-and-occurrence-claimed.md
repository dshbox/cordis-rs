# Events are typed, completion-aware, and occurrence-claimed

Status: accepted; destructor passages amended by
[ADR 0041](0041-user-destructor-panics-are-best-effort.md)

An Event is one named typed contract: within one Runtime, one name binds
one compatible Args/Output contract, declared by a marker trait carrying
only `NAME`, `Args`, and `Output` with no supertraits. Private TypeId use
may enforce the contract but never routes the Event; a conflicting
definition fails as `EventContractMismatch` before any incompatible
callback work. Bounds are minimal: universal Send/Sync/static and payload
Clone/Sync requirements are removed, fan-out and query operations require
`Args: Clone` only where they duplicate the payload, `waterfall`
transports one owned, possibly move-only Args, and Output is moved and
never universally Clone.

Exactly four primitive operations remain. Each is awaited — completion
means all owned work finished — and each takes an explicit `Routing`:

- `emit`: ordered awaited notification over a fixed eligible snapshot in
  effective listener order; successful Observer/Responder outputs are
  ignored; it fails first, and return means all work before that failure
  completed.
- `emit_parallel`: claims the complete invocation set before any
  callback, starts and awaits every claimed invocation, and aggregates
  all failures into one guaranteed non-empty `ParallelFailures` in
  effective listener order, never completion order.
- `query`: sequential first-answer ask; Observer is NoAnswer, the first
  explicit `Answer(Output)` wins, and `QueryOutcome::Miss` makes absence
  explicit, so false, zero, empty text, and empty collections are
  answers.
- `waterfall`: an owned Mapper/Around onion transporting one Args value
  through to a mandatory tail.

`parallel` became `emit_parallel`; `serial` and `bail` collapsed into
`query`; all scoped twins and aliases are removed. `waterfall_query` is
derived composition, not a fifth primitive or store: its mandatory tail
runs one `query` under the same Routing and hands the full Miss/Answer or
failure to the resolver, whose own failure routes outward through the
Around layers.

Within a waterfall, the first effective listener is outermost. A Mapper
transforms and delegates exactly once. An Around may transform,
post-process, handle a downstream failure, or veto the inner layers and
the tail by not consuming its consuming, single-use `Next<E>`; consuming
`Next` prevents retry and fan-out, and the tail runs at most once. A
returned error or contained panic propagates outward through the Around
layers, where an outer Around may replace or recover it; failure never
rolls back earlier user effects or already committed framework state.
An unused tail remains operation-owned on preflight failure, and an
uncalled continuation remains operation-owned when a Mapper fails.

_Amended by [ADR 0041](0041-user-destructor-panics-are-best-effort.md)._ This
passage previously promised specific outcomes when the destructor of an unused
tail or uncalled continuation panicked. ADR 0041 deliberately withdraws those
promises. Destruction of these operation-owned values now follows its general
best-effort rule, and the existing containment remains as best-effort
robustness, not contract.

Listener roles are semantic protocol input, not closure shapes.
`Listener<E>` is sealed and methodless, and the registration/storage
protocol and wrapper types are private; consumers use the role adapters
`observer`/`observer_sync`, `responder`/`responder_sync`,
`mapper`/`mapper_sync`, and `around`. Sync means immediate completion and
may still return a typed error; Around has no sync adapter because its
role may await consuming `Next<E>`. `with_state(factory, callback)`
returns an opaque `StatefulCallback` composing with every role; it
constructs invocation-local state exactly once per delivered invocation,
only after routing, role preflight, and a successful claim, outside
locks, and a factory panic is that listener's invocation failure.

Role compatibility is preflighted before any claim: ordered and parallel
notification accept Observer and Responder, query accepts both with
Observer as NoAnswer, and waterfall accepts Mapper and Around. A known
incompatible snapshot fails with `DispatchError::IncompatibleRole` before
callbacks, claims, or state factories run; preflight failure claims
nothing and consumes no once occurrence.

Every successful registration is one exact occurrence, independent of
callback equality, Event, Fiber, Scope, or options; the gated commit
publishes the occurrence and its generation-owned cleanup atomically.
`ListenerRegistration` is move-only with inert Drop, and its consuming
`remove() -> bool` competes with generation cleanup for one exact
unregister claim. Generation gate close freezes new registration while
existing occurrences stay eligible until their own LIFO unregister
cleanup runs. Remove-before-invocation-claim skips future delivery;
invocation-claim-before-remove completes owned work; removal can never
cancel or join an in-flight callback or remove a later duplicate
occurrence. A once occurrence is consumed — unregistered — atomically
immediately before its winning callback; at most one dispatch or remove
contender wins it, callback failure or panic does not restore it, and
query short-circuit, outer waterfall veto, a lost claim, and role
preflight failure before reaching it all leave it registered. Losing a
claim is a neutral skip: notification and query continue, waterfall
delegates downstream, and no mismatch, failure, answer, or state
allocation results, so concurrent dispatches never double-deliver one
once occurrence.

Routing is explicit per operation. `Routing::Scoped(scope)` selects
global registrations plus those whose registration Scope is
ancestor-or-self of the dispatch Scope; siblings and descendants are
excluded. `Routing::Unscoped` considers every Scope. A global
registration widens eligibility without changing cleanup ownership.
Callback Context attribution comes from registration, never from the
emitter's routing position, and a claimed callback continues even if the
registering Fiber closes.

Every public Event operation is completion-aware: its returned future is
never silently detached, unregister controls only future claims, and
operation completion awaits all work it already owns. Detached delivery
is reserved for Runtime observation. Listener returned errors and panics
are contained at invocation and normalized exactly once into an opaque
`InvocationFailure` — `ReturnedError` or `Panic`, owned diagnostic text,
and the exact registration id when applicable — by the last adapter that
knows the concrete error type; the original object, `Any`, and downcast
never escape, and panic containment covers future polling and state
factories. Cancellation can destroy a pending invocation's callback
while dropping the operation future, without producing an
`InvocationFailure`. What happens next follows the active primitive's
fail-first, attempt-all, or onion rule.

_Amended by [ADR 0041](0041-user-destructor-panics-are-best-effort.md)._ This
decision previously also promised that destruction of a completed invocation's
final callback reference was contained and correlated. A destructor panic
failed the invocation, discarded any returned value, and appended its
diagnostic to an earlier failure. ADR 0041 deliberately withdraws those
promises. A destructor panic may still be reported as a `Panic` invocation
failure on a best-effort basis, but Events make no destructor-specific
promise.

## Rationale

Explicit answer presence, completion ownership, role compatibility, and
exact claims remove truthiness guessing, floating work, callback-shape
inference, and once/remove races — without exposing store or continuation
machinery. Naming the contract and awaiting every primitive makes each
operation's completion a fact the caller can rely on; making registration
occurrences exact and claiming them at invocation turns delivery,
removal, and once semantics into closed arbitrations instead of races
between aliases.

## Consequences

- Inferred closure shapes and paired methods have no v3 behavior;
  migration picks role constructors plus an explicit Routing.
- Once/remove races resolve to one claim arbitration: either removal wins
  and no delivery occurs, or the invocation claim wins and the callback
  completes as framework-owned work.
- `waterfall_query` adds no Event, store, or primitive surface; its
  resolver observes the full query outcome under the caller's Routing.
- Dispatching formerly public internal Events with ordinary Event
  primitives has no v3 behavior: update control and Runtime observation
  are not dispatchable Events.

## Non-normative lineage

| V3 rule | Lineage classification | Historical evidence only |
| --- | --- | --- |
| Named typed Events and completion-aware dispatch algebra | collapses several historical ADRs into one final rule | ADRs 0003 and 0006 |
| Exact listener lifecycle, claim-at-invocation, and registration-Context attribution | collapses several historical ADRs into one final rule | ADRs 0007, 0009, 0014, and 0015 |
| Owned waterfall with derived waterfall-query composition | supersedes part of an existing ADR | ADR 0020 |
