# cordis-rs

**English** | [简体中文](README.zh-CN.md)

[![CI](https://github.com/dshbox/cordis-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/dshbox/cordis-rs/actions/workflows/ci.yml)

A runtime-agnostic Rust port of Cordis 4.x — the plugin framework at the core of [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness), vendored there as [`@deepseek-ai/cordis`](https://github.com/deepseek-ai/deepseek-harness/tree/master/vendor/cordis).

> This implementation is based on Cordis 4.0.1 from DeepSeek Harness. Its core structure mirrors the original `Context / Events / Fiber / Logger / Reflect / Registry / Service` modules and preserves automatic activation when dependencies arrive, automatic unloading when dependencies disappear, scoped isolation, effect cleanup, and all five event dispatch modes as closely as Rust allows.

Cordis is a context-based plugin framework for applications that need explicit dependency injection, scoped services, lifecycle-managed cleanup, structured events, and configuration-driven plugins. `cordis-rs` preserves that model while replacing JavaScript-only mechanisms (Proxy, prototype inheritance, callable objects, decorators, and `any`) with explicit Rust APIs, `Arc`, and checked downcasts.

## Status

The crate currently ports the complete **core runtime**:

| TypeScript Cordis | Rust API | Status |
| --- | --- | --- |
| `new Context()` / `extend()` | `Context::new()` / `extend()` | ✅ |
| `isolate()` and shared labels | `isolate()` / `isolate_with()` | ✅ |
| `intercept()` | `intercept()` / `intercepts()` | ✅ |
| Proxy-backed `get/set/provide` | typed `get/require/set/provide` | ✅ |
| Accessor and mixin reflection | `accessor()` and explicit `alias()` | ✅¹ |
| Function/object/class plugins | `Plugin`, `plugin_sync`, `plugin_async`, service adapters | ✅ |
| `inject` dependency epochs | `Inject` and automatic unload/reload | ✅ |
| `FiberState`, `try_wait`, `restart`, `update`, `dispose` | same lifecycle operations | ✅ |
| Sync/async/generator effects | sync/async disposers and nested effect handles | ✅² |
| `emit/parallel/serial/bail/waterfall` | same five dispatch modes | ✅ |
| Context listener filters | `with_filter()` / `emit_from()` | ✅ |
| Logger buffer/exporters/levels/formatters | corresponding logger APIs | ✅ |
| Standard Schema validation | `Plugin::validate_config` + validation issues | ✅³ |
| `internal/plugin`, `internal/status`, `internal/service`, `internal/dispatch` | same meta-events | ✅⁴ |
| Intercept meta-events (`internal/get`/`set`/`config`/`update`/`listener`) | not ported | Not included |
| Decorators and callable services | explicit Rust traits/builders | Rust-native |
| Loader / include packages | [`cordis-include`](../crates/cordis-include), [`cordis-group`](../crates/cordis-group), [`cordis-loader`](../crates/cordis-loader), [`cordis-cli`](../crates/cordis-cli) | ✅ separate crates |

1. Rust cannot dynamically project arbitrary struct fields like a JavaScript Proxy, so `alias()` is the explicit counterpart to common `mixin()` usage.
2. Rust plugin code registers multiple effects explicitly; `EffectHandle::adopt()` provides the original nested diagnostic/disposal tree.
3. Validation is trait-based because Standard Schema is a JavaScript protocol.
4. `internal/dispatch` carries `(mode, name, args)`; the upstream fourth `thisArg` argument is omitted. The waterfall/bail interception points used by upstream HMR and config injection (`internal/get`, `internal/set`, `internal/config`, `internal/update`, `internal/listener`) are not part of this port, so downstream code relying on them needs a different extension point.

## Design goals

- **Faithful lifecycle:** a plugin remains `Pending` until every injected service is active. Replacing/removing a provider changes the dependency epoch, unloads the consumer, and starts it again when possible.
- **Scoped DI:** isolated branches resolve different implementations of the same service. Reusing an `Isolation` label joins scopes.
- **Ownership-based cleanup:** plugins, listeners, services, exporters, accessors, and child plugins are effects of their creating fiber.
- **No executor lock-in:** the crate has no third-party dependencies. Futures are accepted through boxed standard-library futures; eager lifecycle operations use a small wake-aware executor.
- **Type-checked dynamic values:** service, config, and event storage uses `Value` (`Arc<dyn Any + Send + Sync>`) with checked downcasting and useful type errors.

## Install

```sh
cargo add cordis-rs
```

```toml
[dependencies]
cordis-rs = "0.4"
```

The package is published as `cordis-rs`; the library crate is still named `cordis`, so imports remain `use cordis::...`.

The minimum supported Rust version (MSRV) is **Rust 1.85**, and the crate uses **Rust 2024 Edition**. The crate has no external dependencies.

## Rust version policy

- **MSRV:** Rust 1.85. CI and releases must continue to compile and test on this exact version.
- **Development toolchain:** the latest stable Rust release is used for formatting, Clippy, documentation, and forward-compatibility testing.
- **Review cadence:** the MSRV is reviewed every six months, around February and August. A review does not imply an automatic version increase.
- **Review factors:** maintainers consider the compiler shipped by stable Linux distributions, requirements of official plugins and downstream projects, useful language or standard-library improvements, dependency/security constraints, and toolchain versions actually used by downstream users.
- **Version changes:** the MSRV is raised only when there is a concrete maintenance or ecosystem benefit. An increase is documented in the changelog and release notes and is made in a minor release, never silently in a patch release.
- **Workspace consistency:** official Cordis crates and plugins should use one shared MSRV unless a documented platform constraint requires an exception.

## Quick start

```rust
use cordis::{plugin_sync, Context, Inject, LogArg, PluginOutput, Result, Service};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct Counter(AtomicUsize);

impl Service for Counter {
    const NAME: &'static str = "counter";
}

fn main() -> Result<()> {
    let root = Context::new();
    let counter = Arc::new(Counter(AtomicUsize::new(0)));
    let _provider = root.provide_service_arc(counter.clone())?;

    let greeter = plugin_sync::<(), _>(
        "greeter",
        Inject::new(["counter"]),
        |ctx, _config| {
            let counter = ctx.require::<Counter>("counter")?;
            let value = counter.0.fetch_add(1, Ordering::SeqCst) + 1;
            ctx.logger().info(
                "%s #%d",
                [LogArg::from("started"), LogArg::from(value)],
            );
            Ok(PluginOutput::none())
        },
    );

    let fiber = root.plugin_default(greeter);
    fiber.try_wait()?;
    assert_eq!(counter.0.load(Ordering::SeqCst), 1);

    fiber.dispose()?;
    root.fiber()?.dispose()?;
    Ok(())
}
```

## Dependency injection and reload

`Inject` controls whether a plugin may be active. Service changes reconcile consumers immediately and deterministically.

```rust
use cordis::{plugin_sync, Context, FiberState, Inject, PluginOutput, Result};

fn main() -> Result<()> {
    let root = Context::new();
    let consumer = plugin_sync::<(), _>(
        "consumer",
        Inject::new(["database"]),
        |ctx, _| {
            println!("database = {}", *ctx.require::<String>("database")?);
            Ok(PluginOutput::infallible(|| println!("consumer unloaded")))
        },
    );

    let fiber = root.plugin_default(consumer);
    assert_eq!(fiber.state(), FiberState::Pending);

    let database = root.provide("database", "sqlite://app.db".to_owned())?;
    assert_eq!(fiber.state(), FiberState::Active);

    database.dispose()?;
    assert_eq!(fiber.state(), FiberState::Pending);
    Ok(())
}
```

A plugin can attach per-service intercept config as part of its inject declaration:

```rust
use cordis::{Inject, LoggerIntercept, LoggerLevel};

let inject = Inject::new(["database"]).require_with(
    "logger",
    LoggerIntercept {
        name: Some("worker".into()),
        level: Some(LoggerLevel::Debug),
    },
);
```

## Scoped services

```rust
use cordis::{Context, Result};

fn main() -> Result<()> {
    let root = Context::new();
    let label = root.new_isolation();
    let tenant_a = root.isolate_with("cache", label);
    let tenant_a_worker = root.isolate_with("cache", label);
    let tenant_b = root.isolate("cache");

    let _cache = tenant_a.provide("cache", String::from("A"))?;
    assert_eq!(tenant_a_worker.require::<String>("cache")?.as_str(), "A");
    assert!(tenant_b.get::<String>("cache")?.is_none());
    assert!(root.get::<String>("cache")?.is_none());
    Ok(())
}
```

## Effects

Every effect is single-shot and fiber-owned. Fiber unloading runs effects in reverse registration order. Cleanup errors are logged and do not prevent the remaining effects from running.

```rust
use cordis::{Context, Result};

let root = Context::new();
let handle = root.effect_infallible("temporary file", || {
    // remove the file
})?;

assert_eq!(handle.meta().label, "temporary file");
handle.dispose()?; // early cleanup
handle.dispose()?; // no-op
# Ok::<(), cordis::CordisError>(())
```

Use `effect_async()` or `AsyncDisposer::from_async()` for asynchronous cleanup. A child plugin, listener, provided service, logger exporter, or accessor is internally registered as the same kind of effect.

## Events

Arguments and bail values are `Value`s. `None` means “continue”; `Some(value)` means “bail”.

```rust
use cordis::utils::block_on;
use cordis::{Context, Result, Value};

let root = Context::new();
let _listener = root.on("math/double", |event| {
    let input = event.arg::<u32>(0)?.unwrap();
    Ok(Some(Value::new(*input * 2)))
})?;

let answer = root.events()
    .bail("math/double", [Value::new(21_u32)])?
    .unwrap()
    .downcast::<u32>()?;
assert_eq!(*answer, 42);

block_on(root.events().parallel("tick", []))?;
# Ok::<(), cordis::CordisError>(())
```

Dispatch modes:

- `emit`: invoke in order and synchronously propagate the first error.
- `parallel`: poll every listener concurrently and aggregate errors.
- `serial`: await in order and stop on the first bail value.
- `bail`: synchronous ordered bail.
- `waterfall` / `waterfall_async`: each listener receives `event.next()` and may wrap or veto the rest of the chain.

## Reflection

Normal Rust code should prefer typed services. `Value`, `Accessor`, and `alias()` support dynamic framework/loader use cases:

```rust
use cordis::{Accessor, Context, Result, Value};
use std::sync::{Arc, Mutex};

let root = Context::new();
let state = Arc::new(Mutex::new(1_u32));
let read = state.clone();
let write = state.clone();

let _property = root.accessor("answer", Accessor::read_write(
    move |_| Ok(Some(Value::new(*read.lock().unwrap()))),
    move |_, value| {
        *write.lock().unwrap() = *value.downcast::<u32>()?;
        Ok(())
    },
))?;

root.set("answer", 42_u32)?;
assert_eq!(*root.require::<u32>("answer")?, 42);
# Ok::<(), cordis::CordisError>(())
```

## Logger

The logger keeps a bounded chronological buffer and sends structured `Message`s to effect-owned exporters. It supports Cordis placeholders (`%s`, `%d`, `%i`, `%f`, `%o`, `%O`, `%c`, `%C`, and `%%`), per-name levels, custom formatters, ANSI name colors, and logger intercepts.

```rust
use cordis::{default_format, Context, ExporterConfig, LogArg, LoggerLevel, Result};

let root = Context::new();
let mut config = ExporterConfig::default();
config.levels.insert("default".into(), LoggerLevel::Debug);
let render = config.clone();
let _exporter = root.logger_service().exporter_fn(config, move |message| {
    println!("{}", default_format(&render, message));
})?;

root.named_logger("app").info("listening on %d", [LogArg::from(8080)]);
# Ok::<(), cordis::CordisError>(())
```

## Writing a custom plugin

Closure adapters cover most plugins. Dynamic loaders can implement the object-safe trait directly:

```rust
use cordis::utils::BoxFuture;
use cordis::{Config, Context, Inject, Plugin, PluginOutput, Result};

struct Worker {
    inject: Inject,
}

impl Plugin for Worker {
    fn name(&self) -> &str { "worker" }
    fn inject(&self) -> &Inject { &self.inject }

    fn apply(&self, ctx: Context, _config: Config)
        -> BoxFuture<Result<PluginOutput>>
    {
        Box::pin(async move {
            let _queue = ctx.require::<String>("queue")?;
            Ok(PluginOutput::none())
        })
    }
}
```

Override `validate_config()` to normalize config or return `CordisError::validation(...)`. `service_sync()` and `service_async()` adapt constructors returning a type that implements `Service`.

## Runtime notes

The original TypeScript implementation schedules lifecycle work through promises. This crate deliberately reconciles lifecycle transitions eagerly: `provide`, effect disposal, `restart`, and `update` return after affected fibers settle. This makes behavior deterministic without requiring Tokio or another executor. `Fiber::await_ready`, async event modes, async plugins, and async disposers remain available.

Executor-independent futures work everywhere. If a future creates runtime-specific resources (for example `tokio::time::sleep`), call Cordis while that runtime is entered.

Two consequences of the eager model: `Fiber::try_wait()` reports the settled state instead of suspending until dependencies arrive — it returns an error for `Pending` or disposed fibers. And futures driven by Cordis run on a small blocking executor while a lifecycle transition lock is held, so plugin `apply` callbacks and disposers must only await work that completes on other threads (never same-thread channels or `spawn_blocking` joins).

`Fiber::update()` mirrors upstream on inactive fibers: on an `Active` fiber it validates the new config, restarts, and reports the startup outcome; on a `Pending` or `Failed` fiber it stores the config and reconciles without waiting, so `Ok(())` only means the config was accepted — inspect `state()`/`error()` for the outcome of the activation it schedules.

A panic in a plugin `apply`, disposer, or event listener propagates to the caller of the lifecycle operation that triggered it. Internal mutexes recover from poisoning, and a fiber interrupted mid-transition stays in `Loading`/`Unloading` — with already-registered effects still owned — until the next lifecycle event or `dispose` settles it. `Context` and `Fiber` do not implement `UnwindSafe` because their trait objects cannot prove it; when a plugin must not take down its caller, isolate it with `std::panic::catch_unwind(std::panic::AssertUnwindSafe(...))`.

## Ecosystem

The core crate stays dependency-free; the loader stack lives in sibling
crates that build on it:

| Crate | Purpose |
| --- | --- |
| [`cordis-include`](../crates/cordis-include) | Config entry trees, YAML/JSON loader files, `${{ env.NAME }}` interpolation, atomic and debounced writes |
| [`cordis-group`](../crates/cordis-group) | Group plugin: nested entries with cascading disable |
| [`cordis-loader`](../crates/cordis-loader) | Plugin registry + entry↔fiber state machine, cross-file `import` entries, hot reload, lifecycle events, debounced write-backs, dynamic-library plugins (`dynamic` feature) |
| [`cordis-cli`](../crates/cordis-cli) | `cordis run` executable: daemon/worker exit-code protocol, signals, dotenv, plugin-library hot restarts |

Ported so far: static plugin registry, groups, `import` sub-files, self-kill
detection, entry-level inject, config hot reload, the `loader/*` event
family, debounced writes, the daemon/worker runner, and dynamic-library
plugins with worker-restart HMR (`cordis-loader`'s `dynamic` feature plus
`cordis run --plugin-dir`). Not yet ported: isolate / service migration.

## Project layout

The repository is a virtual cargo workspace; the core crate lives in
`crates/cordis` and mirrors the upstream package:

```text
crates/
├── cordis/            # cordis-rs — this crate (zero dependencies)
│   └── src/
│       ├── context.rs   # root/child context and scope overlays
│       ├── events.rs    # event bus and five dispatch modes
│       ├── fiber.rs     # plugin lifecycle and effect ownership
│       ├── logger.rs    # messages, formatters, buffer, exporters
│       ├── reflect.rs   # scoped service store and computed properties
│       ├── registry.rs  # Plugin, Inject, runtime records
│       ├── service.rs   # typed service and constructor adapters
│       ├── effect.rs    # disposers, handles, diagnostic trees
│       ├── value.rs     # Arc<dyn Any> values
│       └── utils.rs     # boxed futures, small executor
├── cordis-include/    # entry trees and config files
├── cordis-group/      # group plugin
├── cordis-loader/     # plugin registry + state machine
└── cordis-cli/        # cordis run executable
```

## Development

```sh
# MSRV compatibility
cargo +1.85 check --workspace --all-targets --all-features
cargo +1.85 test --workspace --all-features

# Latest stable quality and forward-compatibility checks
cargo +stable fmt --all -- --check
cargo +stable clippy --workspace --all-targets --all-features -- -D warnings
cargo +stable test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo +stable doc --workspace --no-deps --all-features
```

## License

MIT. The architecture and behavior are based on Cordis by Shigma and the DeepSeek Harness vendored implementation.
