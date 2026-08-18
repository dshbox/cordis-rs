# cordis-loader

[English](README.md) | **简体中文**

为 [cordis-rs](https://crates.io/crates/cordis-rs) 插件框架提供由配置文件
驱动的插件加载器。

本 crate 是移植上游 Cordis loader 的组装层：它把
[cordis-include](https://crates.io/crates/cordis-include) 的条目树接到
cordis fiber 上，并在此之上重新导出所需的一切（`cordis-include`、
`cordis-group`），应用只需依赖本 crate 即可。

```text
┌─ cordis-loader   ← 本 crate：插件注册表 + fiber 状态机
├─ cordis-group    group 插件（嵌套标记）
├─ cordis-include  条目树 + 配置文件
└─ cordis-rs       核心运行时（零依赖）
```

## 示例

```rust
use cordis::{plugin_sync, Inject, PluginOutput};
use cordis_include::{Document, EntryOptions, Node};
use cordis_loader::{Loader, LoaderConfig, PluginRegistry};

# fn main() -> cordis_loader::Result<()> {
// 按名字注册插件（上游动态 import() 的替代方案）。
// 工厂为每个条目产出一个全新的 handle。
let mut registry = PluginRegistry::new();
registry.register("greeter", || {
    plugin_sync::<Node, _>(
        "greeter",
        Inject::default(),
        |_ctx, config| {
            let port = config["port"].as_i64().unwrap_or(80);
            println!("greeter on port {port}");
            Ok(PluginOutput::none())
        },
    )
});

let path = std::env::temp_dir().join(format!("cordis-loader-readme-{}.yml", std::process::id()));
let initial = Document::with_entries(vec![
    EntryOptions::new("greeter")
        .with_id("greet")
        .with_config([("port".to_string(), Node::Int(8080))].into_iter().collect()),
]);

let root = cordis::Context::new();
let loader = Loader::open(
    &root,
    LoaderConfig::new(&path).with_registry(registry).with_initial(initial),
)?;

let entry = loader.tree().resolve("greet").unwrap();
entry.fiber().unwrap().try_wait()?;      // 从文件启动
loader.update_config("greet", [("port".to_string(), Node::Int(9090))].into_iter().collect())?;
entry.fiber().unwrap().try_wait()?;      // 以新配置重启

loader.dispose()?;
let _ = std::fs::remove_file(&path);
# Ok(())
# }
```

## Imports（子文件挂载）

一个 `name: import`、`config: { url: "…" }` 的条目会把另一个配置文件挂载
为自己的子树——同一套 diff 机制、同样的 id 复用：

```yaml
# main.yml
entries:
  - id: extra
    name: import
    config:
      url: extra.yml
```

```yaml
# extra.yml —— 这些条目成为 `extra` 的子条目
entries:
  - id: adapter
    name: adapter-http
```

重载时所有相关文件会被组合成一次整树 diff；写回时挂载的条目总是路由回
它们来源的文件（包括生成的 id）。import 环会被记录到 `last_error()` 并
跳过。

## 文档组合源

[`LoaderConfig::with_document`] 从内存文档组合，而不是读取条目文件；
[`Loader::update`] 以同样的方式协调一次全新组合 —— 分层装配模型：
先离线组合各层，再挂载结果。boot 因此既不读也不写该文件（同一份共享
草稿上的并发 boot 不会竞态，目录也可以保持只读），reload 从存储的文档
重新组合（import 文件仍会读取），`update` 不落任何盘 —— 文件只是纯
写回草稿：

```rust
use cordis::{plugin_sync, Inject, PluginOutput};
use cordis_include::{Document, EntryOptions, Node};
use cordis_loader::{Loader, LoaderConfig, PluginRegistry};

# fn main() -> cordis_loader::Result<()> {
# let mut registry = PluginRegistry::new();
# registry.register("worker", || {
#     plugin_sync::<Node, _>("worker", Inject::default(), |_, _| Ok(PluginOutput::none()))
# });
let path = std::env::temp_dir().join(format!("cordis-loader-doc-{}.yml", std::process::id()));
let _ = std::fs::remove_file(&path);
let root = cordis::Context::new();
let loader = Loader::open(
    &root,
    LoaderConfig::new(&path).with_registry(registry).with_document(Document::with_entries(
        vec![EntryOptions::new("worker").with_id("w1")],
    )),
)?;
assert!(!path.exists(), "boot touched the draft");

// 重组合：完整 diff → stop → patch → start，不写回。
let diff = loader.update(Document::with_entries(vec![
    EntryOptions::new("worker").with_id("w1").with_config(
        [("port".to_string(), Node::Int(8080))].into_iter().collect(),
    ),
]))?;
assert_eq!(diff.updated.len(), 1);
assert!(!path.exists(), "recomposition touched the draft");

// 草稿只在写回时被触碰：
loader.update_config("w1", [("port".to_string(), Node::Int(9090))].into_iter().collect())?;
assert!(path.exists(), "write-back landed in the draft");

loader.dispose()?;
let _ = std::fs::remove_file(&path);
# Ok(())
# }
```

## 动态库插件

启用 `dynamic` feature 后，条目还可以解析为编译成 `cdylib` 库的插件——
Rust 没有稳定 ABI，因此加载前会做严格的构建指纹校验（cordis-rs 版本、
精确到 commit hash 的 rustc 版本、目标三元组、panic 策略）：凡不是由
加载进程自己的工具链构建的库都会被拒绝，而不是引发未定义行为。

插件侧：一个 `crate-type = ["cdylib"]`、带 `dynamic` feature 依赖
`cordis-loader` 的 crate 实现 `Plugin`，并以导出宏结尾（文件名决定插件
名）：

```rust,ignore
// greeter-plugin/src/lib.rs —— 构建产物为 libgreeter.so /
// libgreeter.dylib / greeter.dll
use cordis_loader::dynamic::{BoxFuture, Config, Context, Plugin, PluginOutput, Result};

pub struct Greeter;

impl Plugin for Greeter {
    fn name(&self) -> &str { "greeter" }

    fn apply(&self, _ctx: Context, _config: Config) -> BoxFuture<Result<PluginOutput>> {
        Box::pin(async { Ok(PluginOutput::none()) })
    }
}

cordis_loader::dynamic::export_plugin!(Greeter);
```

加载侧：把目录挂到注册表上；静态注册表中找不到的名字会从该目录下的
`lib<name>.so`（macOS 为 `.dylib`，Windows 为 `<name>.dll`）解析：

```rust
# use cordis_loader::PluginRegistry;
let registry = PluginRegistry::new().with_dynamic_dirs(["/usr/lib/cordis-plugins"]);
# assert!(registry.names().any(|name| name == "group"));
```

每次解析都会让库产出一个挂在全新 handle 上的全新插件实例。panic 会被
限制在插件侧——`cdylib` 链接了自己的 std，导出宏给每个回调都包了一层
守卫，在 panic 越过边界之前把它转成错误和兜底值。库在一个进程内绝不
卸载，替换库文件需要全新进程——这正是 `cordis-cli` 用
`cordis run --plugin-dir <dir>` 驱动的 HMR 流程：它监视这些目录，库一
变更就让 worker 热重启（退出码 51）。完整安全模型见 `dynamic` 模块文档。

## 状态机做了什么

- **open** —— 读取（或创建）条目文件，构建条目树，启动每个启用的条目；
  group 的子条目启动在其 group fiber 的 context 之下，因此销毁 group 会
  级联。主文件损坏或不可读会让 open 失败，而不是悄悄启动一棵空树。
- **reload** —— 重新读取文件并协调（与 `update_config`、`dispose` 互斥
  串行）：新建条目启动，移除的子树停止，移动的条目在新父节点下重启，
  插件名 / inject 声明 / 启用标志变化的条目以新选项停止再启动，纯配置
  变化原地 patch——被插件拒绝的 patch 会让 fiber 保留旧配置，并在下一
  次 reload 时重试。之后会持久化新生成的 id，让下一次 reload 能匹配
  它们。
- **文档组合源** —— `LoaderConfig::with_document` 从内存文档 boot
  （文件变成纯写回草稿，绝不再作为组合输入），`Loader::update` 用同一
  套机制协调一次全新组合且不写回 —— 分层组合的 HMR 原语。文档组合源
  loader 的 reload 从存储的文档重新组合；只有 import 文件会被重读。
- **表达式** —— `!!js` 标量（配置值与 `disabled` 槽位）在激活时经
  cordis-include 的 `process.*` 子集求值；disabled 表达式决定该条目
  （及其子树）是否启动，子集外或求失败的表达式使该条目启动失败并记
  录进 `last_error()`（patch 路径上 fiber 保留当前配置，下一次 reload
  重试）。文件与 dump 始终保留表达式原文。
- **dispose** —— 停止每个条目，停止文件监视，并释放 loader 的根级
  effect（状态监听器和 `loader` 服务）；之后在同一 root 上重新
  `Loader::open` 依然可用。
- **self-kill（自杀）** —— 在 loader 操作之外到达 `Disposed` 的 fiber
  是被它自己的插件杀掉的；loader 会在稍后为该条目持久化
  `disabled: true`（持久化被推迟到垂死 fiber 的 transition 锁之外）。
  从文件中删除条目则只是停止它。
- **inject** —— 条目的 `inject` 列表会与插件自己的声明合并（两者共同
  把关启动），因此服务的增删会通过核心机制协调条目。import 图必须是
  树：环与重复挂载同一文件会以不同的诊断记录到 `last_error()`。
- **update_config** —— 运行时修改配置的入口：更新 fiber 并持久化到
  文件。

## 事件与写合并

loader 会在根 context 的事件总线上发出 `loader/entry-init`、
`loader/before-patch`、`loader/after-patch`、`loader/partial-dispose` 和
`loader/config-update`——每个监听器都会收到受影响的条目，
`config-update` 还会收到新配置节点。写防抖会合并高频写回：

```rust
# use cordis_loader::{Loader, LoaderConfig};
# use std::time::Duration;
# let root = cordis::Context::new();
# let config = LoaderConfig::new("cordis.yml").with_write_debounce(Duration::from_millis(300));
let loader = Loader::open(&root, config)?;
// 如今 loader.update_config(...) 的调用会在 300ms 的静默后
// 合并成一次磁盘写入；loader.file().flush_deferred() 可以等它落地。
# Ok::<(), cordis_loader::LoaderError>(())
```

## Feature flags

- **`watch`** —— 热重载：把 `LoaderFile` 的防抖监视器接到
  [`Loader::reload`] 上。重载错误记录在 `last_error()`。
- **`dynamic`** —— 从动态库插件解析条目（`libloading`）：通过
  `PluginRegistry::with_dynamic_dirs` 做指纹校验加载，为插件 crate 提供
  `export_plugin!` 宏，并在插件侧限制 panic。

[`cordis-cli`](https://crates.io/crates/cordis-cli) 运行器在本 crate 之上
构建了 `cordis run` 命令。

[`Loader::reload`]: https://docs.rs/cordis-loader/latest/cordis_loader/struct.Loader.html#method.reload
[`Loader::update`]: https://docs.rs/cordis-loader/latest/cordis_loader/struct.Loader.html#method.update
[`LoaderConfig::with_document`]: https://docs.rs/cordis-loader/latest/cordis_loader/struct.LoaderConfig.html#method.with_document
