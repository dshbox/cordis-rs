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
