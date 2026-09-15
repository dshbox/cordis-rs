# cordis-loader

`cordis-loader` is the declarative loading layer for Cordis v3. It turns
validated immutable source declarations into prepared Cordis Plugins and then
executes them with one complete ordered outcome per plan entry.

The public flow is deliberately explicit:

```text
serialized source
      │
      ▼
LoadPlanBuilder ──validate/freeze──> LoadPlan
                                      │
                           synchronous PluginResolver
                                      │
                              PreparedPlugin
                                      │
                              Context::spawn
                                      │
                                      ▼
                                  LoadOutcome
```

The v3 line is a replacement architecture. It began at `0.1.x` and currently
publishes on `0.2.x`; it is **not** a source-compatible continuation of the
legacy `cordis-loader 0.0.x` API.

## Installation

For an application using the `cordis` facade:

```toml
[dependencies]
cordis-rs = "0.8"
cordis-loader = "0.2"
serde_json = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Framework integrations may depend on `cordis-core = "0.2"` directly instead of
`cordis-rs`.

## Minimal complete load

A resolver receives only declarative resolution input. It prepares and seals the
Plugin before Cordis lifecycle admission; Loader then performs the spawn and
records the result for that exact plan entry.

```rust
use std::convert::Infallible;

use cordis::{BoxError, Context, Plugin, PreparedPlugin};
use cordis_loader::plan::PluginEntry;
use cordis_loader::resolver::{PluginRequest, prepare_plugin_json};
use cordis_loader::LoadPlanBuilder;
use serde_json::Value;

struct Worker;

impl Plugin for Worker {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, _ctx: Context, _input: &()) -> Result<(), Infallible> {
        println!("worker started");
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let mut builder = LoadPlanBuilder::new();
    builder.add_plugin(
        None,
        PluginEntry {
            key: Some("worker".into()),
            name: None,
            config: Value::Null,
            disabled: false,
            inject: Vec::new(),
            isolate: Vec::new(),
        },
    )?;
    let plan = builder.finish()?;

    let resolver = |request: PluginRequest<'_>| -> Result<Option<PreparedPlugin>, Infallible> {
        Ok(match request.resolve_key() {
            "worker" => Some(
                prepare_plugin_json(Worker, request.config())
                    .expect("the plan contains valid Worker configuration"),
            ),
            _ => None,
        })
    };

    let ctx = Context::new();
    let outcome = plan.load(&ctx, &resolver).await;
    assert!(outcome.is_ok());

    // Delivered FiberHandles are consumer-owned. Drop is inert; dispose explicitly.
    for fiber_handle in outcome.fiber_handles() {
        fiber_handle.dispose().await?;
    }

    Ok(())
}
```

Real resolvers normally match several resolve keys and use
`prepare_plugin_json` / `prepare_service_json` to terminate raw JSON at the
Loader boundary.

## What Loader owns

### Immutable plan construction

`LoadPlanBuilder` validates declarations as they are admitted and `finish()`
freezes the plan. `EntryId` values are opaque correlation identities belonging to
one plan lineage; the frozen backing topology is not a public navigation API.

Plugin rows can declare:

- a resolution identity (`key`, falling back to `name`);
- required/configured Service dependencies;
- per-entry Service isolation policy;
- explicit disabled state.

Structural groups provide sequencing only. They are not Plugins, Service realms,
or lifecycle owners.

### Synchronous resolution and preparation

`PluginResolver` is intentionally synchronous and receives no `Context` or
Runtime topology capability. `Some(PreparedPlugin)` means the target has already
been typed, prepared, dependency-overlaid, and sealed. `None` means an unknown
resolve key. Resolver errors and panics are normalized into per-entry Loader
failures.

This creates a hard firewall: malformed JSON or target-specific preparation
failure is handled before `Context::spawn`, so invalid source does not first
become a failed resident Fiber.

### Per-execution Service placement

Loader owns declarative textual `RealmPolicy::{Private, Shared { label }}`.
Shared labels rendezvous only within one `LoadPlan::load` execution. Core sees
only opaque Runtime-local `ServiceRealm` values; textual labels never become a
core-global namespace.

### Complete outcomes

`LoadPlan::load` is partial rather than all-or-nothing. Every frozen plan entry
produces exactly one ordered `EntryOutcome`: structural group, disabled, pruned,
spawned, or failed. Independent failures do not prevent later reachable entries
from being attempted.

`LoadOutcome::fiber_handles()` exposes successfully delivered FiberHandles in outcome order.
After delivery they are caller-owned and must be disposed explicitly.

## What changed from legacy `0.0.x`

The v3 Loader intentionally removes the old representation-heavy surface. In
particular, consumers should not expect a mutable/navigable public entry tree,
erased `DynPlugin`/`Any` configuration, last-wins resolve-key lookup, or the
legacy dynamic-loader/file-watcher modules.

Migration is semantic rather than mechanical: build an immutable `LoadPlan`,
adapt source into typed prepared targets in a resolver, then consume the complete
`LoadOutcome`.

## Compatibility and MSRV

`cordis-loader` v3 began at `0.1.x`; the current `0.2.x` line depends on
`cordis-core 0.2.x`, requires **Rust 1.88 or newer**, and uses Rust edition 2024.

## Documentation

- [Cordis repository](https://github.com/dshbox/cordis-rs)
- [v3 migration guide](https://github.com/dshbox/cordis-rs/blob/main/MIGRATION.md)
- [Loader architecture](https://github.com/dshbox/cordis-rs/blob/main/docs/v3-architecture.md)
- [v3 Loader public interface](https://github.com/dshbox/cordis-rs/blob/main/docs/v3-public-interface.md#loader-plan-and-source-schema)
- [v3 migration inventory](https://github.com/dshbox/cordis-rs/blob/main/docs/v3-migration.md#loader)

Licensed under MIT.
