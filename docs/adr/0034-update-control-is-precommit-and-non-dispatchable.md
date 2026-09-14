# Update control is precommit and non-dispatchable

Status: accepted

Same-Fiber update policy is a typed, framework-invoked, scoped
Mapper/Around pipeline over a request-local candidate. The candidate is a
sealed, move-only, one-attempt `PreparedChange` associated with one
Plugin contract; it carries no Plugin behavior and no lifecycle
authority. Consumers register policy with
`Context::on_update<P>(listener, options)`, which accepts only compatible
`mapper`, `mapper_sync`, or `around` adapters as a sealed,
non-dispatchable `UpdateListener<P>` with an opaque consuming
`UpdateNext<P>`; Observer and Responder roles fail at the type boundary.
No public Event primitive can invoke an update listener: consumers may
subscribe policy, but only the framework's `update` operation runs the
pipeline, and update control can never be dispatched or forged through
Event machinery.

Each pipeline routes internally as `Routing::Scoped` to the target Fiber:
registrations at the target's Scope or an ancestor participate, siblings
do not, and the `global` listener option widens eligibility by the normal
Event rule. Mapper and Around layers may transform, veto, or fail the
candidate, and an outer Around may recover a precommit downstream control
failure — but every accepted value is provisional. The pipeline's
mandatory private tail captures the accepted typed candidate and performs
no config, generation, settlement, or other lifecycle commit. Callback
success and tail acceptance are separate facts: success without a
surviving tail candidate is `UpdateOutcome::Vetoed`, and reaching the
tail proves provisional acceptance only — it commits nothing by itself.
Veto or precommit error never returns or replays the consumed
`PreparedChange`; the old config and generation remain intact.

Admission is revalidated after awaited control completes and before
lifecycle commit: `UpdateError::AdmissionLost` leaves the old state
intact, and only the final accepted candidate crosses the commit. A valid
`PreparedChange` sealed for Plugin contract Q submitted to a FiberHandle for
contract P fails precommit with `UpdateError::PluginContractMismatch`,
leaving config, generation, and FiberId untouched. A committed update
returns `UpdateOutcome::Committed(FiberState)` with a state that is only
`Active` or stable `Pending`; `Vetoed` is normal precommit policy
refusal.

The pipeline ends at the commit boundary. Postcommit apply failure
retains the new authoritative Plugin input, parks the target as Failed, and
reaches the caller as `UpdateError::Apply`; control callbacks cannot
observe or recover it. Era replacement never invokes the update pipeline:
a successor's configuration enters through the era-swap protocol, not
through same-Fiber policy.

## Rationale

Keeping update policy precommit lets it transform or veto a candidate
without acquiring lifecycle authority, holding the lifecycle arbiter over
user awaits, or disguising a committed failure as precommit control.
Keeping the pipeline outside Event dispatch — sealed listener types, a
private mandatory tail, framework-only invocation — prevents consumers
from forging updates and keeps "the framework asked, policy answered"
distinct from "anyone emitted an event."

## Consequences

- Veto and precommit failure are cheap and total: the old config and
  generation survive, the consumed candidate is gone, and no replay
  occurs.
- Recovery from a committed apply failure is lifecycle policy — restart
  or era replacement — never an Around layer catching `Apply`; the
  pipeline provably cannot observe postcommit failure.
- Era-swap policy hooks have no v3 behavior: `on_update` registrations
  never fire for a successor's creation.
- The erased public `InternalUpdate` Event and its downcast protocol have
  no v3 behavior; typed contract association replaces raw config
  downcasts.

## Non-normative lineage

| V3 rule | Lineage classification | Historical evidence only |
| --- | --- | --- |
| Typed precommit update control outside lifecycle commit | supersedes part of an existing ADR | ADR 0018 |
