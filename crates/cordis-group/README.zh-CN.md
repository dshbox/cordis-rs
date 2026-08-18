# cordis-group

[English](README.md) | **简体中文**

为 [cordis-rs](https://crates.io/crates/cordis-rs) 插件框架提供带级联禁用的
嵌套插件分组。

group 是配置文件中携带 `group` 数组的条目。运行时 loader 会把每个 group
作为一个 [`Group`] fiber 启动，并把子条目启动在**该 fiber 的 context
之下**：销毁 group 会级联到整棵子树，而祖先链上任意位置的 `disabled`
标志都会让整棵子树从一开始就不启动。

```yaml
entries:
  - id: staging
    name: group
    disabled: true      # 级联到每个子条目
    group:
      - name: adapter-http
```

## 示例

```rust
use cordis::{plugin_sync, Inject, PluginOutput, FiberState};
use cordis_group::Group;
# fn main() -> cordis::Result<()> {
let root = cordis::Context::new();
let group = root.plugin_default(Group::handle());
group.try_wait()?;

// 在 group 的 context 下启动的子条目会随它一起消亡。
let group_ctx = group.context().unwrap();
let child = group_ctx.plugin_default(plugin_sync::<(), _>(
    "child", Inject::default(), |_, _| Ok(PluginOutput::none()),
));
child.try_wait()?;

group.dispose()?;
assert_eq!(child.state(), FiberState::Disposed);
# Ok(())
# }
```

插件本身只是一个 no-op 的嵌套标记——全部编排逻辑（遍历条目树、在正确的
context 下启动子条目、patch 配置）都位于
[`cordis-loader`](https://crates.io/crates/cordis-loader)，它会在自己的插件
注册表里预注册 `group` 这个名字。

[`Group`]: https://docs.rs/cordis-group/latest/cordis_group/struct.Group.html
