# cordis-core

`cordis-core` is the semantic runtime contract of Cordis v3. It provides the
lifecycle, dependency, event, cleanup, logging, and observation primitives used
by Cordis applications and ecosystem crates.

Use this crate directly when you are writing a framework integration, Plugin
library, or other code that should depend on the Cordis runtime contract rather
than the application-facing compatibility facade.

## Installation

```toml
[dependencies]
cordis-core = "0.3"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Application authors can instead depend on `cordis-rs = "0.8"`. That package
keeps the historical Rust import path `use cordis::...` and re-exports the
`cordis-core` public surface.

## Minimal lifecycle

A Plugin prepares typed source configuration before lifecycle admission. A
`PreparedPlugin` seals the Plugin/input pair, and `Context::spawn` returns a
`FiberHandle` only after the new Fiber reaches its current stable state.

```rust
use std::convert::Infallible;

use cordis_core::{BoxError, Context, Plugin, PreparedPlugin};

struct Greeter;

impl Plugin for Greeter {
    type Config = String;
    type Input = String;
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, config: String) -> Result<Self::Input, Self::PrepareError> {
        Ok(config)
    }

    async fn apply(
        &self,
        _ctx: Context,
        input: &Self::Input,
    ) -> Result<(), Self::ApplyError> {
        println!("hello, {input}");
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let ctx = Context::new();
    let plugin = Greeter;
    let input = plugin.prepare("world".into())?;
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(plugin, input))
        .await?;

    fiber_handle.dispose().await?;
    Ok(())
}
```

Dropping a `FiberHandle` does not dispose its Fiber. Lifecycle ownership is explicit;
call `dispose()` when the consumer is finished with it.

## What `cordis-core` owns

The crate deliberately groups the runtime around semantic responsibilities:

- **Context** — an immutable view into one Cordis Runtime.
- **Plugin / FiberHandle / Fiber lifecycle** — prepare, spawn, ready, restart, update,
  era replacement, and deterministic disposal.
- **Services** — typed capabilities published into exact `(Service,
  ServiceRealm)` slots with explicit dependency convergence.
- **Events** — typed observer/responder/mapper/around roles with explicit
  `Routing::Unscoped` or `Routing::Scoped(scope)` dispatch.
- **Generation-owned resources** — listeners, Service publications, tasks,
  effects, exporters, and cleanup obligations live with the apply generation
  that registered them.
- **Runtime observation and logging** — read-only semantic snapshots and
  postcommit observation, never an alternate control plane.

A `Context` carries independent Service-isolation, Event-Scope, and intercept
axes. Service placement does not imply Event reachability, and Scope ancestry
does not imply lifecycle ownership.

## What it deliberately does not own

`cordis-core` stays free of application policy and optional leaf concerns:

- no serde/JSON loading policy;
- no production Tokio time-driver dependency;
- no file watching, dynamic Plugin discovery, or process-level orchestration;
- no hidden Context hierarchy that simultaneously controls lifecycle, Services,
  and Events.

Those boundaries keep the runtime reusable while allowing optional crates to
build on the same semantic contract.

## Related crates

| Crate | Role |
|---|---|
| `cordis-rs` `0.8.x` | application-facing facade; Rust crate name remains `cordis` |
| `cordis-timer` `0.3.x` | generation-owned sleep, interval, and timeout operations |
| `cordis-loader` `0.3.x` | immutable declarative load plans and typed resolution |

## Compatibility and MSRV

The v3 semantic crates began at `0.1.x` and now publish on `0.3.x`. The
application-facing `cordis-rs` line entered v3 at `0.7.x` and now publishes on
`0.8.x`. Cordis v3 requires **Rust 1.88 or newer** and uses
Rust edition 2024.

Cordis is pre-1.0. Semantic compatibility is documented explicitly, but minor
versions may still contain intentional public API changes.

## Documentation

- [Cordis repository](https://github.com/dshbox/cordis-rs)
- [v3 architecture](https://github.com/dshbox/cordis-rs/blob/main/docs/v3-architecture.md)
- [v3 public interface](https://github.com/dshbox/cordis-rs/blob/main/docs/v3-public-interface.md)
- [0.6.x → v3 migration guide](https://github.com/dshbox/cordis-rs/blob/main/MIGRATION.md)

Licensed under MIT.
