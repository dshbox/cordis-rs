# cordis-rs

[English](README.md) | **简体中文**

[![CI](https://github.com/dshbox/cordis-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/dshbox/cordis-rs/actions/workflows/ci.yml)

[DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) 底座插件框架 Cordis 4.x 的运行时无关 Rust 移植（在其仓库中内置为 [`@deepseek-ai/cordis`](https://github.com/deepseek-ai/deepseek-harness/tree/master/vendor/cordis)）。

> 本实现以 DeepSeek Harness 中的 Cordis 4.0.1 为原版，核心结构与原版的 `Context / Events / Fiber / Logger / Reflect / Registry / Service` 模块对应，并在 Rust 语言能力允许的范围内，尽量保留依赖到达时自动激活、依赖消失时自动卸载、作用域隔离、effect 回收，以及全部五种事件分发模式。

Cordis 是一个基于上下文的插件框架，适用于需要显式依赖注入、作用域服务、生命周期资源回收、结构化事件和配置驱动插件的应用。`cordis-rs` 保留了这一运行模型，同时将 JavaScript 特有的机制（Proxy、原型继承、可调用对象、装饰器和 `any`）替换为显式 Rust API、`Arc` 和经过检查的向下类型转换。

## 当前状态

本 crate 已移植完整的**核心运行时**：

| TypeScript Cordis | Rust API | 状态 |
| --- | --- | --- |
| `new Context()` / `extend()` | `Context::new()` / `extend()` | ✅ |
| `isolate()` 和共享标签 | `isolate()` / `isolate_with()` | ✅ |
| `intercept()` | `intercept()` / `intercepts()` | ✅ |
| 基于 Proxy 的 `get/set/provide` | 类型化的 `get/require/set/provide` | ✅ |
| Accessor 和 mixin 反射 | `accessor()` 和显式 `alias()` | ✅¹ |
| 函数/对象/类插件 | `Plugin`、`plugin_sync`、`plugin_async`、service 适配器 | ✅ |
| `inject` 依赖 epoch | `Inject` 和自动卸载/重载 | ✅ |
| `FiberState`、`try_wait`、`restart`、`update`、`dispose` | 对应的生命周期操作 | ✅ |
| 同步/异步/generator effect | 同步/异步 disposer 和嵌套 effect handle | ✅² |
| `emit/parallel/serial/bail/waterfall` | 相同的五种分发模式 | ✅ |
| 上下文监听器过滤 | `with_filter()` / `emit_from()` | ✅ |
| Logger 缓冲区/exporter/级别/格式化器 | 对应的 logger API | ✅ |
| Standard Schema 校验 | `Plugin::validate_config` + 校验问题列表 | ✅³ |
| `internal/plugin`、`internal/status`、`internal/service`、`internal/dispatch` | 同名元事件 | ✅⁴ |
| 拦截元事件（`internal/get`/`set`/`config`/`update`/`listener`） | 未移植 | 未包含 |
| 装饰器和可调用服务 | 显式 Rust trait/builder | Rust 原生实现 |
| Loader / include 包 | [`cordis-include`](../crates/cordis-include)、[`cordis-group`](../crates/cordis-group)、[`cordis-loader`](../crates/cordis-loader)、[`cordis-cli`](../crates/cordis-cli) | ✅ 独立 crate |

1. Rust 无法像 JavaScript Proxy 一样动态投影任意 struct 字段，因此 `alias()` 是常见 `mixin()` 用法的显式对应方案。
2. Rust 插件代码会显式注册多个 effect；`EffectHandle::adopt()` 提供与原版对应的嵌套诊断和回收树。
3. Standard Schema 是 JavaScript 协议，因此 Rust 版本采用 trait 方式进行校验。
4. `internal/dispatch` 携带 `(mode, name, args)` 三个参数，省略了上游的第四个 `thisArg` 参数。上游 HMR 和配置注入依赖的 waterfall/bail 拦截点（`internal/get`、`internal/set`、`internal/config`、`internal/update`、`internal/listener`）不在本移植范围内，依赖这些事件的下游代码需要另找扩展点。

## 设计目标

- **忠实的生命周期：** 插件会保持 `Pending`，直到所有注入服务都处于活动状态。替换或移除 provider 会改变依赖 epoch、卸载 consumer，并在条件再次满足时重新启动。
- **作用域 DI：** 不同隔离分支可以解析同一服务的不同实现。复用 `Isolation` 标签可以让不同分支加入同一作用域。
- **基于所有权的回收：** 插件、监听器、服务、exporter、accessor 和子插件都是创建它们的 fiber 所拥有的 effect。
- **不绑定执行器：** crate 不包含第三方依赖。它通过标准库 boxed future 接受异步任务；即时生命周期操作由一个小型、支持 wake 的执行器驱动。
- **经过类型检查的动态值：** 服务、配置和事件存储使用 `Value`（`Arc<dyn Any + Send + Sync>`），通过经过检查的向下转换提供清晰的类型错误。

## 安装

```sh
cargo add cordis-rs
```

```toml
[dependencies]
cordis-rs = "0.4"
```

包以 `cordis-rs` 名称发布；库 crate 名仍为 `cordis`，导入方式保持 `use cordis::...` 不变。

最低支持 Rust 版本（MSRV）为 **Rust 1.85**，并使用 **Rust 2024 Edition**。该 crate 没有外部依赖。

## Rust 版本策略

- **MSRV：** Rust 1.85。CI 和发布流程必须持续使用这个确切版本完成编译和测试。
- **开发工具链：** 使用最新 stable Rust 执行格式化、Clippy、文档生成和向前兼容性测试。
- **评估周期：** 每六个月评估一次 MSRV，时间安排在每年 2 月和 8 月前后。评估不代表一定会提高版本。
- **评估因素：** 维护者会考虑稳定版 Linux 发行版自带的编译器、官方插件和下游项目的要求、有价值的语言或标准库改进、依赖及安全限制，以及下游用户实际使用的工具链版本。
- **版本变更：** 只有存在明确的维护或生态收益时才提高 MSRV。提高版本必须记录在 changelog 和 release notes 中，并通过 minor 版本发布，绝不在 patch 版本中静默变更。
- **Workspace 一致性：** 除非存在有文档说明的平台限制，否则官方 Cordis crate 和插件应使用统一的 MSRV。

## 快速开始

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

## 依赖注入和重载

`Inject` 控制插件能否进入活动状态。服务变化会立即、确定性地协调 consumer 状态。

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

插件可以在 inject 声明中附加针对各服务的 intercept 配置：

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

## 作用域服务

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

## Effect

每个 effect 都只能执行一次，并由 fiber 所拥有。Fiber 卸载时会按照注册顺序的逆序执行 effect。回收错误会被记录，但不会阻止其余 effect 继续执行。

```rust
use cordis::{Context, Result};

let root = Context::new();
let handle = root.effect_infallible("temporary file", || {
    // 删除文件
})?;

assert_eq!(handle.meta().label, "temporary file");
handle.dispose()?; // 提前回收
handle.dispose()?; // 无操作
# Ok::<(), cordis::CordisError>(())
```

异步回收可以使用 `effect_async()` 或 `AsyncDisposer::from_async()`。子插件、监听器、已提供的服务、logger exporter 或 accessor 在内部都会注册为同一种 effect。

## 事件

参数和 bail 值都是 `Value`。`None` 表示“继续”，`Some(value)` 表示“退出分发”。

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

分发模式：

- `emit`：按顺序调用，并同步返回第一个错误。
- `parallel`：并发轮询所有监听器并聚合错误。
- `serial`：按顺序等待，在遇到第一个 bail 值时停止。
- `bail`：同步、按顺序执行的 bail 分发。
- `waterfall` / `waterfall_async`：每个监听器都会收到 `event.call_next()`，并可以包装或阻止后续调用链。

## 反射

普通 Rust 代码应优先使用类型化服务。`Value`、`Accessor` 和 `alias()` 用于支持动态框架或 loader 场景：

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

Logger 会维护一个有界、按时间顺序排列的缓冲区，并将结构化 `Message` 发送给由 effect 所拥有的 exporter。它支持 Cordis 占位符（`%s`、`%d`、`%i`、`%f`、`%o`、`%O`、`%c`、`%C` 和 `%%`）、按名称配置的级别、自定义格式化器、ANSI 名称颜色和 logger intercept。

```rust
use cordis::{default_format, Context, ExporterConfig, LogArg, LoggerLevel, Result};

let root = Context::new();
let mut config = ExporterConfig::default();
config.levels.insert("default".into(), LoggerLevel::Debug);
let render = config.clone();
let _exporter = root.logger_service().exporter_with(config, move |message| {
    println!("{}", default_format(&render, message));
})?;

root.named_logger("app").info("listening on %d", [LogArg::from(8080)]);
# Ok::<(), cordis::CordisError>(())
```

## 编写自定义插件

闭包适配器可以满足大多数插件。动态 loader 可以直接实现对象安全的 trait：

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

可以重写 `validate_config()` 来规范化配置，或返回 `CordisError::validation(...)`。`service_sync()` 和 `service_async()` 可以将返回 `Service` 实现类型的构造器适配为插件。

## 运行时说明

TypeScript 原版通过 Promise 调度生命周期任务。本 crate 特意采用即时生命周期协调：`provide`、effect 回收、`restart` 和 `update` 会在受影响的 fiber 稳定后才返回。因此，无需 Tokio 或其他执行器也能获得确定性行为。同时仍然提供异步事件模式、异步插件和异步 disposer；其中 `dispose_async` 是同步直通实现，从不让出执行器——await 期间会阻塞调用线程直至完成。

与执行器无关的 future 可以在任何环境中运行。如果 future 会创建特定运行时资源（例如 `tokio::time::sleep`），请在对应运行时已经进入的情况下调用 Cordis。

即时模型带来两个后果：`Fiber::try_wait()` 报告的是已稳定的状态，而不是挂起等待依赖到达——对 `Pending` 或已销毁的 fiber 它会返回错误。此外，Cordis 驱动 future 时使用一个小型阻塞执行器，且持有生命周期迁移锁，因此插件的 `apply` 回调和 disposer 只能等待在其他线程上完成的工作（不得等待同线程的 channel 或 `spawn_blocking` 的 join）。

`Fiber::update()` 在非活动 fiber 上与上游一致：对 `Active` fiber 它会先校验新配置、再重启，并报告启动结果；对 `Pending` 或 `Failed` fiber 它只保存配置并立即协调，不等待激活——此时 `Ok(())` 仅表示配置已被接受，激活结果需要通过 `state()`/`error()` 观察。

插件 `apply`、disposer 或事件监听器中的 panic 会传播给触发生命周期操作的调用方。内部互斥锁会从 poisoning 中恢复；在迁移途中被打断的 fiber 会停留在 `Loading`/`Unloading` 状态（已注册的 effect 仍归其所有），直到下一次生命周期事件或 `dispose` 使其稳定。`Context` 和 `Fiber` 未实现 `UnwindSafe`（其内部的 trait object 无法证明该性质）；当某个插件不允许拖垮调用方时，请在调用处用 `std::panic::catch_unwind(std::panic::AssertUnwindSafe(...))` 进行隔离。

## 生态

核心 crate 保持零依赖；loader 栈位于其上的兄弟 crate 中：

| Crate | 用途 |
| --- | --- |
| [`cordis-include`](../crates/cordis-include) | 配置条目树、YAML/JSON 配置文件、补丁列表与溯源导出（bundle/profile 组合）、保真 `!!js` 方言与表达式求值、`${{ env.NAME }}` 插值、原子写与防抖合并写 |
| [`cordis-group`](../crates/cordis-group) | 分组插件：嵌套条目与级联禁用 |
| [`cordis-loader`](../crates/cordis-loader) | 插件注册表 + 条目↔fiber 状态机、跨文件 `import` 条目、文档组合源（`with_document` / `update`）、配置热重载、生命周期事件、防抖写回、动态库插件（`dynamic` feature） |
| [`cordis-cli`](../crates/cordis-cli) | `cordis run` 可执行入口：daemon/worker 退出码协议、信号、dotenv、插件库热重启 |

已移植：静态插件注册表、分组、`import` 子文件、自杀判别、entry 级
inject、配置热重载、`loader/*` 事件族、防抖写、bundle/profile 补丁组合
与溯源导出（`apply_entry_patches` / `compose_layers` /
`render_config_dump`）、保真 `!!js` YAML 方言（表达式子集在配置交接时
求值，含 `disabled: !!js` 槽位）、文档组合源与内存重组合、
daemon/worker 运行器，以及动态库插件 + worker 重启式 HMR
（`cordis-loader` 的 `dynamic` feature 与 `cordis run --plugin-dir`）。
尚未移植：isolate / 服务迁移、超出出厂子集的任意 JavaScript 表达式。

## 项目结构

仓库是虚拟 cargo workspace；核心 crate 位于 `crates/cordis`，源码结构与上游 package 对应：

```text
crates/
├── cordis/            # cordis-rs —— 本 crate（零依赖）
│   └── src/
│       ├── context.rs   # root/child 上下文和作用域覆盖
│       ├── events.rs    # 事件总线和五种分发模式
│       ├── fiber.rs     # 插件生命周期和 effect 所有权
│       ├── logger.rs    # 消息、格式化器、缓冲区、exporter
│       ├── reflect.rs   # 作用域服务存储和计算属性
│       ├── registry.rs  # Plugin、Inject、运行时记录
│       ├── service.rs   # 类型化服务和构造器适配器
│       ├── effect.rs    # disposer、handle、诊断树
│       ├── value.rs     # Arc<dyn Any> 动态值
│       └── utils.rs     # boxed future、小型执行器
├── cordis-include/    # 条目树与配置文件
├── cordis-group/      # 分组插件
├── cordis-loader/     # 插件注册表 + 状态机
└── cordis-cli/        # cordis run 可执行入口
```

## 开发

```sh
# MSRV 兼容性
cargo +1.85 check --workspace --all-targets --all-features
cargo +1.85 test --workspace --all-features

# 最新 stable 的质量和向前兼容性检查
cargo +stable fmt --all -- --check
cargo +stable clippy --workspace --all-targets --all-features -- -D warnings
cargo +stable test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo +stable doc --workspace --no-deps --all-features
```

## 许可证

MIT。架构和行为基于 Shigma 的 Cordis，以及 DeepSeek Harness 中内置的 Cordis 实现。
