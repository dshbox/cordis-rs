# cordis-group

**English** | [简体中文](README.zh-CN.md)

Nested plugin groups with cascading disable for the
[cordis-rs](https://crates.io/crates/cordis-rs) plugin framework.

A group is an entry with a `group` array in the config file. At runtime the
loader starts each group as a [`Group`] fiber and starts the child entries
*beneath that fiber's context*: disposing the group cascades to the whole
subtree, and a `disabled` flag anywhere on the ancestor chain keeps the
subtree from starting at all.

```yaml
entries:
  - id: staging
    name: group
    disabled: true      # cascades to every child
    group:
      - name: adapter-http
```

## Example

```rust
use cordis::{plugin_sync, Inject, PluginOutput, FiberState};
use cordis_group::Group;
# fn main() -> cordis::Result<()> {
let root = cordis::Context::new();
let group = root.plugin_default(Group::handle());
group.try_wait()?;

// Children started under the group's context die with it.
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

The plugin itself is a no-op nesting marker — all orchestration (walking
the entry tree, starting children under the right context, patching config)
lives in [`cordis-loader`](https://crates.io/crates/cordis-loader), which
pre-registers the `group` name in its plugin registry.

[`Group`]: https://docs.rs/cordis-group/latest/cordis_group/struct.Group.html
