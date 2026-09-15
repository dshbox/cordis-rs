# Cordis

[English](README.md) | **简体中文**

Cordis v3 是面向长期运行、插件化 Rust 应用的类型化 runtime。它把 Plugin/FiberHandle 生命周期、Service 依赖、类型化 Event、资源清理，以及显式隔离边界放进同一套模型中。

## 当前状态

Cordis v3 现已成为默认 `main` 主线。现有 `0.6.x` 实现保留在 `legacy/0.6` 维护分支，只接受关键 bug 和安全修复。

## 安装

普通应用继续使用历史 package/import identity：

```toml
[dependencies]
cordis-rs = "0.8"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

```rust
use cordis::Context;
```

`cordis-rs` 在 v3 中是很薄的 facade，真正的 runtime contract 位于：

```toml
cordis-core = "0.3"
```

框架和第三方 Plugin 作者可以直接依赖 `cordis-core`。时间与声明式加载能力保持独立：

```toml
cordis-timer = "0.3"
cordis-loader = "0.3"
```

v3 使用 Rust 2024 Edition，MSRV 为 **Rust 1.88**。

## v3 的核心模型

- `Context`：同一个 Runtime 上的轻量不可变视图。
- `Plugin`：可复用行为；`prepare()` 在生命周期准入之前完成输入准备。
- `FiberHandle`：一个已准入非 root Fiber 的生命周期控制 handle。
- `Service` / `ServiceRealm`：Service 解析到精确 realm slot，不存在隐式 fallback。
- `Event` / `Scope`：Event routing 与 Service isolation 是相互独立的 Context axes。
- generation 拥有 cleanup；Runtime/Registry 拥有 Fiber residency。
- update 保持 Fiber identity；era swap 会终止旧 Fiber 并创建新的 identity。

完整模型请阅读英文 [`README.md`](README.md) 和
[`docs/v3-architecture.md`](docs/v3-architecture.md)；1.0 readiness 与 exit criteria
见 [`ROADMAP.md`](ROADMAP.md)。

## 从 0.6.x 迁移

`cordis-rs 0.8.x` 是当前 application-facing v3 发布线；v3 runtime architecture 最初在 `0.7.x` 发布。这不是 `0.6.x` 的 source-compatible 升级。迁移入口：

- [`MIGRATION.md`](MIGRATION.md)：面向现有用户的迁移说明。
- [`docs/v3-migration.md`](docs/v3-migration.md)：更详细的架构迁移 inventory。
- [`docs/v3-public-interface.md`](docs/v3-public-interface.md)：v3 public interface authority。

## Crate

| Crate | 角色 |
|---|---|
| `cordis-rs` | 应用 facade，继续提供 `use cordis::...` |
| `cordis-core` | canonical runtime contract |
| `cordis-timer` | generation-owned sleep / interval / timeout |
| `cordis-loader` | immutable load plans 与 typed target resolution |

## License

MIT。项目延续 Cordis lineage；版权声明见 [`LICENSE`](LICENSE)。
