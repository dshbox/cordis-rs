# Consumer guide

This is practical, non-normative guidance for composing the supported Cordis v3
API. The [glossary](../CONTEXT.md), [architecture](v3-architecture.md),
[public interface](v3-public-interface.md) and accepted ADRs own the contract;
the [compatibility policy](compatibility-policy.md) owns package promises.
Follow those authorities if a summary here is ambiguous.

## 1. Prepare input, seal it, then spawn and retain the handle

Implement `Plugin` directly. Adapt source `Config` into typed `Input` with
`prepare`, seal that value with `PreparedPlugin::from_input`, then await
`Context::spawn`. Preparation is synchronous and precedes lifecycle admission;
a preparation failure or direct panic cannot leave an admitted Fiber. Sealing
proves the Plugin/Input type association, not which particular Plugin object
produced the input. Use concrete Plugin error types; `BoxError` is useful at an
outer application boundary such as `main`, not as the canonical associated error.

Successful spawn hands off a live, quiescent `FiberHandle`: Active means apply
succeeded; Pending means a required Service is unavailable and apply has not
run. Keep the delivered handle for explicit lifecycle control. Dropping it does
not dispose the Fiber. If creation is abandoned after its commit, core owns the
undelivered Fiber's cleanup; a delivered handle belongs in consumer composition.

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
