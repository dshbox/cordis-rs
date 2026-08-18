//! Order-preserving, format-neutral value tree used for entry config.

use indexmap::IndexMap;
use serde::de::{self, MapAccess, SeqAccess};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An ordered string-keyed map of [`Node`]s.
pub type NodeMap = IndexMap<String, Node>;

/// A dynamically typed value that round-trips through YAML and JSON while
/// preserving object key order.
///
/// Entry config read from a file is stored as a `Node`; plugins receive it
/// wrapped in a [`cordis::Value`] and recover it with
/// `downcast::<Node>()`. Unlike `serde_json::Value` or
/// `serde_yaml_ng::Value`, object keys keep their file order, which keeps
/// serialized output stable and friendly to diffs and file watchers.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Node {
    /// Absent value (`null` / `~`).
    #[default]
    Null,
    /// Boolean literal.
    Bool(bool),
    /// Signed integer.
    Int(i64),
    /// Unsigned integer outside `i64` range.
    UInt(u64),
    /// Floating-point number.
    Float(f64),
    /// String literal (subject to `${{ ... }}` interpolation).
    String(String),
    /// A `!!js` expression scalar, kept verbatim and unevaluated: it
    /// round-trips through YAML (`!!js <expr>`) and only takes effect when
    /// config is handed to a plugin. Produced by the crate's own YAML
    /// dialect parser ([`crate::yaml`]); serde paths (JSON) never yield it
    /// and serialize it as its raw text.
    Expr(String),
    /// Array of nodes.
    Array(Vec<Node>),
    /// Ordered object.
    Object(NodeMap),
}

/// Unsigned integers within `i64` range become [`Node::Int`]; only values
/// above it stay [`Node::UInt`]. Parse results and explicit conversions
/// agree, so `Node::Int(1) == 1u64.into()` holds.
fn small_uint(value: u64) -> Node {
    if value <= i64::MAX as u64 {
        Node::Int(value as i64)
    } else {
        Node::UInt(value)
    }
}

impl Serialize for Node {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Null => serializer.serialize_none(),
            Self::Bool(value) => serializer.serialize_bool(*value),
            Self::Int(value) => serializer.serialize_i64(*value),
            Self::UInt(value) => serializer.serialize_u64(*value),
            Self::Float(value) => serializer.serialize_f64(*value),
            Self::String(value) => serializer.serialize_str(value),
            Self::Expr(value) => serializer.serialize_str(value),
            Self::Array(items) => items.serialize(serializer),
            Self::Object(map) => {
                let mut map_serializer = serializer.serialize_map(Some(map.len()))?;
                for (key, value) in map {
                    map_serializer.serialize_entry(key, value)?;
                }
                map_serializer.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NodeVisitor;

        impl<'de> de::Visitor<'de> for NodeVisitor {
            type Value = Node;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("any YAML or JSON value")
            }

            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Node, E> {
                Ok(Node::Bool(value))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Node, E> {
                Ok(Node::Int(value))
            }

            fn visit_i128<E: de::Error>(self, value: i128) -> Result<Node, E> {
                if value >= i64::MIN as i128 && value <= i64::MAX as i128 {
                    Ok(Node::Int(value as i64))
                } else if value >= 0 && value <= u64::MAX as i128 {
                    Ok(Node::UInt(value as u64))
                } else {
                    Err(E::custom("integer out of range"))
                }
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Node, E> {
                Ok(small_uint(value))
            }

            fn visit_u128<E: de::Error>(self, value: u128) -> Result<Node, E> {
                if value <= u64::MAX as u128 {
                    Ok(small_uint(value as u64))
                } else {
                    Err(E::custom("integer out of range"))
                }
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Node, E> {
                Ok(Node::Float(value))
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Node, E> {
                Ok(Node::String(value.to_owned()))
            }

            fn visit_string<E: de::Error>(self, value: String) -> Result<Node, E> {
                Ok(Node::String(value))
            }

            fn visit_none<E: de::Error>(self) -> Result<Node, E> {
                Ok(Node::Null)
            }

            fn visit_unit<E: de::Error>(self) -> Result<Node, E> {
                Ok(Node::Null)
            }

            fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Node, D::Error> {
                deserializer.deserialize_any(self)
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Node, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element::<Node>()? {
                    items.push(item);
                }
                Ok(Node::Array(items))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
                let mut entries = NodeMap::new();
                while let Some((key, value)) = map.next_entry::<String, Node>()? {
                    entries.insert(key, value);
                }
                Ok(Node::Object(entries))
            }
        }

        deserializer.deserialize_any(NodeVisitor)
    }
}

impl Node {
    /// Return the string contents, or `None` for any other node kind.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    /// Return the integer value, or `None` for any other node kind.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            Self::UInt(value) => i64::try_from(*value).ok(),
            _ => None,
        }
    }

    /// Return the boolean value, or `None` for any other node kind.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// Return the object entries, or `None` for any other node kind.
    pub fn as_object(&self) -> Option<&NodeMap> {
        match self {
            Self::Object(map) => Some(map),
            _ => None,
        }
    }

    /// Return the array items, or `None` for any other node kind.
    pub fn as_array(&self) -> Option<&[Node]> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    /// Whether this node is [`Node::Null`].
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }
}

impl From<bool> for Node {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i8> for Node {
    fn from(value: i8) -> Self {
        Self::Int(value as i64)
    }
}

impl From<i16> for Node {
    fn from(value: i16) -> Self {
        Self::Int(value as i64)
    }
}

impl From<i32> for Node {
    fn from(value: i32) -> Self {
        Self::Int(value as i64)
    }
}

impl From<isize> for Node {
    fn from(value: isize) -> Self {
        Self::Int(value as i64)
    }
}

impl From<u8> for Node {
    fn from(value: u8) -> Self {
        Self::Int(value as i64)
    }
}

impl From<u16> for Node {
    fn from(value: u16) -> Self {
        Self::Int(value as i64)
    }
}

impl From<u32> for Node {
    fn from(value: u32) -> Self {
        Self::Int(value as i64)
    }
}

impl From<i64> for Node {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<u64> for Node {
    fn from(value: u64) -> Self {
        small_uint(value)
    }
}

impl From<f64> for Node {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

impl From<usize> for Node {
    fn from(value: usize) -> Self {
        small_uint(value as u64)
    }
}

/// Indexes object keys, panicking only when the node is not an object.
///
/// Missing keys yield [`Node::Null`], which keeps assertions on parsed
/// config concise (`assert_eq!(node["missing"], Node::Null)`).
impl std::ops::Index<&str> for Node {
    type Output = Node;

    fn index(&self, key: &str) -> &Node {
        static NULL: Node = Node::Null;
        self.as_object()
            .and_then(|map| map.get(key))
            .unwrap_or(&NULL)
    }
}

impl From<&str> for Node {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<String> for Node {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<Vec<Node>> for Node {
    fn from(value: Vec<Node>) -> Self {
        Self::Array(value)
    }
}

impl From<NodeMap> for Node {
    fn from(value: NodeMap) -> Self {
        Self::Object(value)
    }
}

impl FromIterator<(String, Node)> for Node {
    fn from_iter<I: IntoIterator<Item = (String, Node)>>(iter: I) -> Self {
        Self::Object(NodeMap::from_iter(iter))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_round_trip_preserves_key_order() {
        let node: Node = serde_yaml_ng::from_str("b: 1\na: 2\nc: [x, y]").unwrap();
        let text = serde_yaml_ng::to_string(&node).unwrap();
        assert!(text.contains("b: 1"), "{text}");
        assert!(text.contains("a: 2"), "{text}");
        let back: Node = serde_yaml_ng::from_str(&text).unwrap();
        assert_eq!(node, back);
    }

    #[test]
    fn json_round_trip_preserves_key_order() {
        let node: Node = serde_json::from_str(r#"{"b":1,"a":2,"c":[1,2.5,true,null]}"#).unwrap();
        let text = serde_json::to_string(&node).unwrap();
        assert_eq!(text, r#"{"b":1,"a":2,"c":[1,2.5,true,null]}"#);
        let back: Node = serde_json::from_str(&text).unwrap();
        assert_eq!(node, back);
    }

    #[test]
    fn wide_integers_survive() {
        let node: Node = serde_yaml_ng::from_str("big: 18446744073709551615").unwrap();
        let Node::Object(map) = &node else {
            panic!("expected object");
        };
        assert_eq!(map["big"], Node::UInt(u64::MAX));
        let text = serde_yaml_ng::to_string(&node).unwrap();
        assert!(text.contains("18446744073709551615"), "{text}");
    }
}
