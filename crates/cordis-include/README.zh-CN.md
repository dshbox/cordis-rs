# cordis-include

[English](README.md) | **简体中文**

为 [cordis-rs](https://crates.io/crates/cordis-rs) 插件框架提供配置条目树
与 YAML/JSON 配置文件。

本 crate 是 Cordis loader 移植的数据层：它在磁盘配置文件与内存条目树之间
做映射，为 diff 友好的文件保留对象键序，展开 `${{ env.NAME }}` 模板，并提供
用于打断 write → watch → write 反馈回路的 suspend guard。

```text
┌─ cordis-loader   组装层：插件注册表 + fiber 状态机
├─ cordis-group    group 插件（嵌套标记）
├─ cordis-include  ← 本 crate：条目树 + 配置文件
└─ cordis-rs       核心运行时（零依赖）
```

## 示例

```rust
use cordis_include::{Entry, EntryOptions, EntryTree, Node};
# fn main() -> cordis_include::Result<()> {
let tree = EntryTree::new();

// 载入一组条目（来自文件，或手工构建）。
let diff = tree.update(vec![
    EntryOptions::new("group").with_id("srv").with_group(vec![
        EntryOptions::new("adapter-http").with_config(
            [("port".to_string(), Node::Int(8080))].into_iter().collect(),
        ),
    ]),
])?;
assert_eq!(diff.created.len(), 2);

// 整树重载按 id 跨 group 匹配：既有条目对象会被复用（同一指针），
// 调用方可以保留自己的句柄。
let kept = tree.resolve("srv").unwrap();
tree.update(vec![EntryOptions::new("group").with_id("srv")])?;
assert!(Entry::ptr_eq(&kept, &tree.resolve("srv").unwrap()));
# Ok(())
# }
```

文件通过 [`LoaderFile`] 往返读写：原子且写者互斥的 `.tmp` + rename 写入
（并发写者不会交错出撕裂内容）、只读检测、未知顶层键原样保留，以及面向
高频调用方的合并延迟写（`write_deferred`）：

```yaml
entries:
  - id: srv
    name: group
    group:
      - name: adapter-http
        config:
          port: 8080
          host: ${{ env.HOST }}
```

## Feature flags

- **`watch`** — 通过 [`notify`](https://crates.io/crates/notify) 实现防抖文件
  监视。文件处于挂起状态期间（由调用方持有的 suspend guard，例如包住调用方
  自己的写入）观察到的事件不会触发回调。

## 范围

本 crate 刻意不去关心*插件从哪里来*，也从不启动或停止 fiber：
[`cordis-loader`](https://crates.io/crates/cordis-loader) 实现这里定义的
[`PluginResolver`] 契约并驱动生命周期。

[`LoaderFile`]: https://docs.rs/cordis-include/latest/cordis_include/struct.LoaderFile.html
[`PluginResolver`]: https://docs.rs/cordis-include/latest/cordis_include/trait.PluginResolver.html
