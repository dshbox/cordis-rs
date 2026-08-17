//! Nested plugin groups with cascading disable for
//! [cordis-rs](https://crates.io/crates/cordis-rs).
//!
//! A group is an entry with a `group` array in the config file (see
//! `cordis-include`). At runtime the loader starts each group entry as a
//! [`Group`] fiber and starts the child entries *beneath that fiber's
//! context*: disposing the group fiber cascades to the whole subtree, and a
//! `disabled` flag anywhere on the ancestor chain keeps the subtree from
//! starting at all (the include tree's `enabled()` walk).
//!
//! The plugin itself is deliberately a no-op nesting marker — upstream
//! cordis' `plugin-group` is similarly tiny. All orchestration lives in
//! `cordis-loader`, which registers [`GROUP_NAME`] in its builtin registry.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use cordis::utils::BoxFuture;
use cordis::{Config, Context, Plugin, PluginHandle, PluginOutput, Result};

/// Entry name that marks a group (`name: group` in the config file).
pub const GROUP_NAME: &str = "group";

/// Nesting marker plugin for group entries.
///
/// Starting a group produces an active fiber whose context scopes the
/// subtree's fibers and effects; the loader is the intended driver.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Group;

impl Group {
    /// A fresh handle around a [`Group`] plugin.
    ///
    /// Each call yields a distinct [`cordis::PluginKey`] identity, matching
    /// one handle per entry.
    pub fn handle() -> PluginHandle {
        PluginHandle::new(Group)
    }
}

impl Plugin for Group {
    fn name(&self) -> &str {
        GROUP_NAME
    }

    fn apply(&self, _ctx: Context, _config: Config) -> BoxFuture<Result<PluginOutput>> {
        Box::pin(async { Ok(PluginOutput::default()) })
    }
}
