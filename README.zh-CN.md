# Cordis

[English](README.md) | **简体中文**

> **文档状态：** 当前中文 README 仅提供简要概览，内容可能滞后于英文版。pre-1.0 阶段请以 [English README](README.md) 为准；接近 1.0 时会再进行一次完整同步。

Cordis v3 是面向长期运行、插件化 Rust 应用的类型化 runtime。它把 Plugin/FiberHandle 生命周期、Service 依赖、类型化 Event、资源清理，以及显式隔离边界放进同一套模型中。

## 当前状态

Cordis v3 现已成为默认 `main` 主线。现有 `0.6.x` 实现保留在 `legacy/0.6` 维护分支，只接受关键 bug 和安全修复。

## 安装

普通应用继续使用历史 package/import identity：

```toml
[dependencies]
cordis-rs = "0.11"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

```rust
use cordis::Context;
```

`cordis-rs` 在 v3 中是很薄的 facade，真正的 runtime contract 位于：

```toml
cordis-core = "0.6"
```

框架和第三方 Plugin 作者可以直接依赖 `cordis-core`。时间与声明式加载能力保持独立：

```toml
cordis-timer = "0.6"
cordis-loader = "0.6"
```

v3 使用 Rust 2024 Edition，MSRV 为 **Rust 1.88**。

从 semantic `0.5` / facade `0.10` 升级时，请将 core、timer、loader 的
requirements 一起更新到 `0.6`，facade 更新到 `0.11`。facade re-export core
类型，混用 `0.5` 和 `0.6` 的 core 类型会导致 type mismatch。`0.6` 是 pre-freeze
breaking 版本线：预期会增长的 public vocabulary 现在是 `#[non_exhaustive]`（需要补
wildcard arm 和 `..` pattern），Loader source row 变为可扩展并改用 `PluginEntry::new`
与 `EntryGroup::new` 构造，另有两条已文档化的承诺被收窄（提交后的创建进度，以及
lifecycle admission order 的含义；见
[ADR 0029](docs/adr/0029-lifecycle-commits-complete-and-critical-sections-are-closed.md)）。
compatible evolution 规则、enum openness 以及 auto trait 与 operation future 的 `Send`
承诺记录在
[ADR 0042](docs/adr/0042-public-evolution-declares-openness-and-auto-traits.md)。
具体修改步骤和更早的升级见
[MIGRATION.md](MIGRATION.md)、
[core release notes](crates/cordis-core/CHANGELOG.md) 和
[facade release notes](crates/cordis/CHANGELOG.md)。

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

`cordis-rs 0.11.x` 是当前 application-facing v3 发布线；v3 runtime architecture 最初在 `0.7.x` 发布。这不是 `0.6.x` 的 source-compatible 升级。迁移入口：

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

## Consumer guide 与 freeze review

使用 API 的十条非规范性指南见 [consumer guide（English）](docs/consumer-guide.md)，
其中链接了现有 authority、runnable examples 与对应 evidence。
[API freeze candidate record（English）](docs/api-freeze-recommendation.md)
记录已宣布的 API freeze candidate（`64aa6de`，#222）与剩余工作；1.0 仍需要一次
不含 planned breaking change 的 stabilization release。
