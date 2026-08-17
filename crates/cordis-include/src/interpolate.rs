//! `${{ env.NAME }}` string templates — the safe stand-in for upstream's
//! `!js` interpolation. No expression evaluation.

use crate::error::{IncludeError, Result};
use crate::node::Node;

/// Expand every `${{ env.NAME }}` template inside the string using process
/// environment variables.
pub fn interpolate_str(input: &str) -> Result<String> {
    interpolate_str_with(input, &resolve_env)
}

/// Recursively expand templates in every string of the value tree using
/// process environment variables.
pub fn interpolate_node(node: &Node) -> Result<Node> {
    interpolate_node_with(node, &resolve_env)
}

/// Recursively expand templates using a caller-supplied lookup (see
/// [`interpolate_str_with`]).
pub fn interpolate_node_with(
    node: &Node,
    lookup: &dyn Fn(&str) -> Result<Option<String>>,
) -> Result<Node> {
    match node {
        Node::String(value) => interpolate_str_with(value, lookup).map(Node::String),
        Node::Array(items) => items
            .iter()
            .map(|item| interpolate_node_with(item, lookup))
            .collect::<Result<Vec<_>>>()
            .map(Node::Array),
        Node::Object(map) => {
            let mut expanded = crate::node::NodeMap::new();
            for (key, value) in map {
                expanded.insert(key.clone(), interpolate_node_with(value, lookup)?);
            }
            Ok(Node::Object(expanded))
        }
        other => Ok(other.clone()),
    }
}

/// Expand templates using a caller-supplied lookup, which maps an
/// expression like `env.NAME` to its value. `Ok(None)` means the
/// expression is well-formed but has no value; an `Err` rejects the
/// expression outright.
pub fn interpolate_str_with(
    input: &str,
    lookup: &dyn Fn(&str) -> Result<Option<String>>,
) -> Result<String> {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("${{") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 3..];
        let end = tail.find("}}").ok_or_else(|| IncludeError::Unterminated {
            input: input.to_owned(),
        })?;
        let expression = tail[..end].trim();
        if expression.is_empty() {
            return Err(IncludeError::UnknownExpression {
                expression: expression.to_owned(),
            });
        }
        let value = lookup(expression)?.ok_or_else(|| IncludeError::MissingEnv {
            expression: expression.to_owned(),
        })?;
        out.push_str(&value);
        rest = &tail[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// The default lookup: `env.NAME` reads `NAME` from the environment.
fn resolve_env(expression: &str) -> Result<Option<String>> {
    match expression.strip_prefix("env.") {
        Some(name) if !name.is_empty() => Ok(std::env::var(name).ok()),
        _ => Err(IncludeError::UnknownExpression {
            expression: expression.to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(expression: &str) -> Result<Option<String>> {
        match expression {
            "env.HOST" => Ok(Some("example.org".to_owned())),
            "env.MISSING" => Ok(None),
            other => Err(IncludeError::UnknownExpression {
                expression: other.to_owned(),
            }),
        }
    }

    #[test]
    fn substitutes_single_and_multiple_templates() {
        let text = interpolate_str_with("host=${{ env.HOST }}:${{ env.HOST }}", &lookup).unwrap();
        assert_eq!(text, "host=example.org:example.org");
    }

    #[test]
    fn plain_strings_pass_through() {
        assert_eq!(
            interpolate_str_with("no templates", &lookup).unwrap(),
            "no templates"
        );
        assert_eq!(interpolate_str_with("", &lookup).unwrap(), "");
    }

    #[test]
    fn missing_and_unknown_expressions_error() {
        assert!(matches!(
            interpolate_str_with("${{ env.MISSING }}", &lookup),
            Err(IncludeError::MissingEnv { .. })
        ));
        assert!(matches!(
            interpolate_str_with("${{ shell.rm }}", &lookup),
            Err(IncludeError::UnknownExpression { .. })
        ));
        assert!(matches!(
            interpolate_str_with("${{ env.MISSING", &lookup),
            Err(IncludeError::Unterminated { .. })
        ));
    }

    #[test]
    fn nodes_recurse() {
        let node: Node = serde_yaml_ng::from_str("url: http://${{ env.HOST }}/api\nn: 1").unwrap();
        let expanded = interpolate_node_with(&node, &lookup).unwrap();
        let map = expanded.as_object().unwrap();
        assert_eq!(map["url"].as_str(), Some("http://example.org/api"));
        assert_eq!(map["n"], Node::Int(1));
    }
}
