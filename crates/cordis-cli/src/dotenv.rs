//! Minimal `.env` loading — `KEY=VALUE` lines, no interpolation.

use std::path::Path;

/// Load `.env` and then `.env.local` from `dir`, without overriding
/// variables that are already set in the environment.
pub fn load(dir: &Path) {
    for name in [".env", ".env.local"] {
        let path = dir.join(name);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (key, value) in parse(&text) {
            if std::env::var_os(&key).is_none() {
                set_if_absent(&key, &value);
            }
        }
    }
}

/// SAFETY: the CLI sets the environment exactly once during single-threaded
/// startup, before any worker threads exist, so no other thread can be
/// reading `std::env` concurrently.
#[allow(unsafe_code)]
fn set_if_absent(key: &str, value: &str) {
    unsafe { std::env::set_var(key, value) };
}

/// Parse dotenv text into key/value pairs. Supports comments, blank lines,
/// an optional `export ` prefix, and single- or double-quoted values.
/// Malformed lines are skipped.
pub fn parse(text: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || key.contains(char::is_whitespace) {
            continue;
        }
        pairs.push((key.to_owned(), unquote(value.trim())));
    }
    pairs
}

/// Strip one layer of matching quotes; `#` starts a comment in unquoted
/// values.
fn unquote(value: &str) -> String {
    let value = if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        &value[1..value.len() - 1]
    } else {
        value
            .split_once('#')
            .map_or(value, |(head, _)| head.trim_end())
    };
    value.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_comments_quotes_and_exports() {
        let text = r#"
# comment
PLAIN=value
export EXPORTED=1
QUOTED="a # b"
SINGLE='c'
TRAILING=x   # trailing comment
EMPTY=
malformed-line
"#;
        assert_eq!(
            parse(text),
            vec![
                ("PLAIN".to_owned(), "value".to_owned()),
                ("EXPORTED".to_owned(), "1".to_owned()),
                ("QUOTED".to_owned(), "a # b".to_owned()),
                ("SINGLE".to_owned(), "c".to_owned()),
                ("TRAILING".to_owned(), "x".to_owned()),
                ("EMPTY".to_owned(), String::new()),
            ]
        );
    }
}
