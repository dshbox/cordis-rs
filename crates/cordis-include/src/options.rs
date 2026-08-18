//! Serializable entry description — the on-disk shape of one plugin entry.

use crate::node::Node;
use serde::{Deserialize, Serialize};

/// One entry's disable state: a static flag or a `!!js` expression
/// evaluated at activation, with the raw text kept for write-back.
///
/// The YAML dialect maps `disabled: true` to [`Disabled::Flag`] and
/// `disabled: !!js <expr>` to [`Disabled::Expr`]; the expression stays
/// unevaluated in the tree and only takes effect through
/// [`crate::Entry::resolved_disabled`]. The serde (JSON) path has no
/// `!!js`, so a flag serializes as a boolean and an expression as its
/// raw string.
#[derive(Debug, Clone, PartialEq)]
pub enum Disabled {
    /// Statically on/off. The default is off.
    Flag(bool),
    /// `disabled: !!js <expr>` — the raw expression, evaluated when the
    /// entry is activated ([`crate::expr::evaluate`]).
    Expr(String),
}

impl Default for Disabled {
    fn default() -> Self {
        Self::Flag(false)
    }
}

impl Disabled {
    /// Whether this statically disables the entry. An unevaluated
    /// expression does not: see [`crate::Entry::resolved_disabled`].
    pub fn is_disabled(&self) -> bool {
        matches!(self, Self::Flag(true))
    }

    /// The raw `!!js` expression text, when the slot holds one.
    pub fn as_expr(&self) -> Option<&str> {
        match self {
            Self::Expr(source) => Some(source),
            _ => None,
        }
    }
}

/// Whether the slot is the default (off), used by `skip_serializing_if`.
fn disabled_is_default(value: &Disabled) -> bool {
    matches!(value, Disabled::Flag(false))
}

impl Serialize for Disabled {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Flag(flag) => serializer.serialize_bool(*flag),
            Self::Expr(source) => serializer.serialize_str(source),
        }
    }
}

impl<'de> Deserialize<'de> for Disabled {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct DisabledVisitor;

        impl serde::de::Visitor<'_> for DisabledVisitor {
            type Value = Disabled;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a boolean or a `!!js` expression string")
            }

            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Disabled, E> {
                Ok(Disabled::Flag(value))
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Disabled, E> {
                Ok(Disabled::Expr(value.to_owned()))
            }

            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Disabled, E> {
                Ok(Disabled::Expr(value))
            }
        }

        deserializer.deserialize_any(DisabledVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The serde (JSON) path: flags are booleans, expressions are their raw
    /// text — JSON has no `!!js`, so a string round-trips the expression.
    #[test]
    fn disabled_serde_json_round_trips() {
        let options: EntryOptions =
            serde_json::from_str(r#"{"name":"n","disabled":true}"#).unwrap();
        assert_eq!(options.disabled, Disabled::Flag(true));
        let options: EntryOptions =
            serde_json::from_str(r#"{"name":"n","disabled":"process.platform"}"#).unwrap();
        assert_eq!(
            options.disabled,
            Disabled::Expr("process.platform".to_owned())
        );
        let text = serde_json::to_string(&options).unwrap();
        assert_eq!(text, r#"{"name":"n","disabled":"process.platform"}"#);
        // The default flag is omitted entirely.
        let text = serde_json::to_string(&EntryOptions::new("n")).unwrap();
        assert_eq!(text, r#"{"name":"n"}"#);
    }
}

/// Entry name that mounts another config file as a subtree
/// (`name: import` with `config: { url: "…" }`).
pub const IMPORT_NAME: &str = "import";

/// Entry name of the built-in group plugin: a row named `group` (or any row
/// with children) is a group.
pub const GROUP_NAME: &str = "group";

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
    /// Whether the entry (and transitively its subtree) is disabled: a
    /// static flag, or a `!!js` expression evaluated at activation whose
    /// raw text round-trips through the file.
    #[serde(default, skip_serializing_if = "disabled_is_default")]
    pub disabled: Disabled,
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

    /// Set the static disable flag (see [`EntryOptions::disabled`]; use the
    /// field directly for a `!!js` expression).
    pub fn with_disabled(mut self, disabled: bool) -> Self {
        self.disabled = Disabled::Flag(disabled);
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
