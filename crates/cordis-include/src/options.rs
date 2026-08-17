//! Serializable entry description — the on-disk shape of one plugin entry.

use crate::node::Node;
use serde::{Deserialize, Serialize};

/// Whether a boolean is `false` (used by `skip_serializing_if`).
fn is_false(value: &bool) -> bool {
    !*value
}

/// Entry name that mounts another config file as a subtree
/// (`name: import` with `config: { url: "…" }`).
pub const IMPORT_NAME: &str = "import";

/// One entry in a config file: a plugin instance plus its group position.
///
/// The declared field order is the serialization order (`id` and `name`
/// first, `config` last), keeping files readable and diff-stable. Entries
/// with a `group` array are groups; the array order is the child order.
///
/// `config` is stored raw: `${{ env.NAME }}` templates stay intact in the
/// entry tree and are only expanded when the config is handed to a plugin
/// (see [`crate::Entry::resolved_config`]).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct EntryOptions {
    /// Stable identity of the entry. Missing ids are filled with a random
    /// 6-character base36 id when the entry enters a tree and persisted on
    /// the next write-back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Plugin name used to resolve the plugin implementation.
    #[serde(default)]
    pub name: String,
    /// Whether the entry (and transitively its subtree) is disabled.
    #[serde(default, skip_serializing_if = "is_false")]
    pub disabled: bool,
    /// Names of services that must be active before this entry starts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inject: Vec<String>,
    /// Child entries, in order. Non-empty only for group entries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub group: Vec<EntryOptions>,
    /// Raw plugin configuration (templates unexpanded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Node>,
}

impl EntryOptions {
    /// Create options for a plugin with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    /// Set the explicit entry id.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the raw plugin configuration.
    pub fn with_config(mut self, config: Node) -> Self {
        self.config = Some(config);
        self
    }

    /// Set the child entries (turning this entry into a group).
    pub fn with_group(mut self, group: Vec<EntryOptions>) -> Self {
        self.group = group;
        self
    }

    /// Mark the entry (and its subtree) as disabled.
    pub fn with_disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The url this import entry mounts, when the entry is an import
    /// (`name: import` with a string `config.url`).
    pub fn import_url(&self) -> Option<&str> {
        if self.name != IMPORT_NAME {
            return None;
        }
        self.config.as_ref()?.as_object()?.get("url")?.as_str()
    }

    /// Declare services that must be active before this entry starts.
    pub fn with_inject<I, S>(mut self, inject: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.inject = inject.into_iter().map(Into::into).collect();
        self
    }
}
