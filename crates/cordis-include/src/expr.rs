//! The `!!js` expression subset evaluated at config hand-off.
//!
//! Upstream evaluates `!!js` scalars with real JavaScript against the
//! loader context. The expressions that actually appear in the shipped
//! bundles only reference `process.*`, so this module evaluates that
//! subset synchronously — at the same hand-off point as the
//! `${{ env.NAME }}` interpolation ([`crate::interpolate`]) — with a
//! hand-written lexer and parser. Expressions referencing injected
//! context (`ctx.*`, `dshHomePath(…)`) are deferred to the owning
//! plugin's lazy evaluation and fail here with a clear subset error.
//!
//! Supported syntax (JavaScript precedence):
//!
//! - the ternary `cond ? then : else`
//! - `??`, `||`, `&&` with JavaScript truthiness (empty strings and zero
//!   are falsy); like JavaScript, `??` may not be mixed with `||`/`&&`
//!   without parentheses
//! - strict equality `===` / `!==` (loose `==`/`!=` is outside the subset)
//! - unary `!`
//! - string literals in single or double quotes, decimal integers and
//!   floats, `true`/`false`/`null`/`undefined`
//! - member access limited to `process.platform`, `process.env.NAME`
//!   (unset variables are `undefined`), and `process.cwd()`
//!
//! Values are [`Node`]s directly; `null` and `undefined` both map to
//! [`Node::Null`], which is also what `??` triggers on.

use crate::error::{IncludeError, Result};
use crate::node::{Node, NodeMap};

/// Evaluate one `!!js` expression, reading environment variables from the
/// process environment.
///
/// # Errors
///
/// Syntax errors, operators or references outside the supported subset,
/// and an unreadable working directory (for `process.cwd()`) fail with
/// [`IncludeError::JsExpression`].
pub fn evaluate(source: &str) -> Result<Node> {
    evaluate_with(source, &|name| std::env::var(name).ok())
}

/// Evaluate one `!!js` expression against a caller-supplied environment
/// lookup, which maps a variable name to its value (`None` when unset).
pub fn evaluate_with(source: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<Node> {
    let mut evaluator = Evaluator { source, env };
    let ast = evaluator.parse()?;
    evaluator.value(&ast)
}

/// Recursively evaluate every [`Node::Expr`] in a value tree, leaving all
/// other nodes untouched — the expression twin of
/// [`crate::interpolate::interpolate_node`], applied when config is
/// handed to a plugin.
pub fn evaluate_node(node: &Node) -> Result<Node> {
    evaluate_node_with(node, &|name| std::env::var(name).ok())
}

/// [`evaluate_node`] with a caller-supplied environment lookup.
pub fn evaluate_node_with(node: &Node, env: &dyn Fn(&str) -> Option<String>) -> Result<Node> {
    match node {
        Node::Expr(source) => evaluate_with(source, env),
        Node::Array(items) => items
            .iter()
            .map(|item| evaluate_node_with(item, env))
            .collect::<Result<Vec<_>>>()
            .map(Node::Array),
        Node::Object(map) => {
            let mut evaluated = NodeMap::new();
            for (key, value) in map {
                evaluated.insert(key.clone(), evaluate_node_with(value, env)?);
            }
            Ok(Node::Object(evaluated))
        }
        other => Ok(other.clone()),
    }
}

// ---------------------------------------------------------------- lexer ---

/// One lexical token; spans are unnecessary because errors name the
/// offending text.
#[derive(Debug, Clone, PartialEq)]
enum Token {
    /// An operator or punctuation this subset supports (`??`, `===`, `(`…).
    Punct(&'static str),
    /// A quoted string literal, unescaped.
    Str(String),
    /// A decimal integer literal.
    Int(i64),
    /// A decimal float literal.
    Float(f64),
    /// An identifier or keyword.
    Ident(String),
    /// Loose equality (`==` / `!=`), rejected by the parser with a
    /// targeted message.
    LooseEq(&'static str),
    /// An operator JavaScript has and this subset does not (`+`, `<`…).
    Unsupported(String),
}

/// Split `source` into tokens.
fn lex(source: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut rest = source;
    while let Some(character) = rest.chars().next() {
        if character.is_whitespace() {
            rest = &rest[character.len_utf8()..];
            continue;
        }
        // Multi-character operators must win over their prefixes (`===`
        // before `==` before `!`).
        let (token, width): (Token, usize) = if rest.starts_with("===") {
            (Token::Punct("==="), 3)
        } else if rest.starts_with("!==") {
            (Token::Punct("!=="), 3)
        } else if rest.starts_with("??") {
            (Token::Punct("??"), 2)
        } else if rest.starts_with("||") {
            (Token::Punct("||"), 2)
        } else if rest.starts_with("&&") {
            (Token::Punct("&&"), 2)
        } else if rest.starts_with("==") {
            (Token::LooseEq("=="), 2)
        } else if rest.starts_with("!=") {
            (Token::LooseEq("!="), 2)
        } else if matches!(character, '?' | ':' | '!' | '(' | ')' | '.') {
            let punct = match character {
                '?' => "?",
                ':' => ":",
                '!' => "!",
                '(' => "(",
                ')' => ")",
                _ => ".",
            };
            (Token::Punct(punct), punct.len())
        } else if character == '\'' || character == '"' {
            match lex_string(rest) {
                Some((text, width)) => (Token::Str(text), width),
                None => (Token::Unsupported(rest.to_owned()), rest.len()),
            }
        } else if character.is_ascii_digit() {
            lex_number(rest)
        } else if character.is_ascii_alphabetic() || character == '_' || character == '$' {
            let ident = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
                .collect::<String>();
            let width = ident.len();
            (Token::Ident(ident), width)
        } else {
            // Anything else is JavaScript this subset does not take.
            (
                Token::Unsupported(character.to_string()),
                character.len_utf8(),
            )
        };
        rest = &rest[width..];
        tokens.push(token);
    }
    tokens
}

/// One quoted string literal with JS-style escapes; `None` when the
/// string is unterminated. Returns the unescaped text and the consumed
/// width.
fn lex_string(rest: &str) -> Option<(String, usize)> {
    let quote = rest.chars().next()?;
    let mut text = String::new();
    let mut width = quote.len_utf8();
    let mut chars = rest[width..].chars();
    while let Some(character) = chars.next() {
        width += character.len_utf8();
        if character == quote {
            return Some((text, width));
        }
        if character == '\n' {
            return None;
        }
        if character == '\\' {
            let escaped = chars.next()?;
            width += escaped.len_utf8();
            text.push(match escaped {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                '0' => '\0',
                // Unknown escapes collapse to the escaped character, like
                // JavaScript string literals.
                other => other,
            });
        } else {
            text.push(character);
        }
    }
    None
}

/// One decimal number: integer when it fits `i64` and has no fraction or
/// exponent, float otherwise.
fn lex_number(rest: &str) -> (Token, usize) {
    fn digits(slice: &str) -> usize {
        slice
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .map(char::len_utf8)
            .sum()
    }
    let mut width = digits(rest);
    let mut is_float = false;
    if rest[width..].starts_with('.') && rest[width + 1..].starts_with(|c: char| c.is_ascii_digit())
    {
        is_float = true;
        width += 1 + digits(&rest[width + 1..]);
    }
    if let Some(tail) = rest[width..].strip_prefix(['e', 'E']) {
        let signed = tail.strip_prefix(['+', '-']).unwrap_or(tail);
        let exponent = digits(signed);
        if exponent > 0 {
            is_float = true;
            width += 1 + (tail.len() - signed.len()) + exponent;
        }
    }
    let text = &rest[..width];
    if !is_float {
        if let Ok(int) = text.parse::<i64>() {
            return (Token::Int(int), width);
        }
    }
    match text.parse::<f64>() {
        Ok(float) => (Token::Float(float), width),
        Err(_) => (Token::Unsupported(text.to_owned()), width),
    }
}

// ----------------------------------------------------------------- ast ---

/// One parsed expression.
enum Ast {
    /// A literal value.
    Literal(Node),
    /// `process.platform`.
    Platform,
    /// `process.env.NAME`.
    Env(String),
    /// `process.cwd()`.
    Cwd,
    /// `!expr`.
    Not(Box<Ast>),
    /// `left ?? right`.
    Coalesce(Box<Ast>, Box<Ast>),
    /// `left || right`.
    Or(Box<Ast>, Box<Ast>),
    /// `left && right`.
    And(Box<Ast>, Box<Ast>),
    /// `left === right` (or `!==` with `negated`).
    StrictEq {
        left: Box<Ast>,
        right: Box<Ast>,
        negated: bool,
    },
    /// `condition ? then : alternative`.
    Ternary {
        condition: Box<Ast>,
        then: Box<Ast>,
        alternative: Box<Ast>,
    },
}

/// Which coalescing/logical operators one parenthesized region used, to
/// reject JavaScript's illegal `??` / `||` / `&&` mixes.
#[derive(Default)]
struct Mixing {
    saw_coalesce: bool,
    saw_logical: bool,
}

/// Parser plus evaluator state: the raw source (for error context) and
/// the environment lookup.
struct Evaluator<'a> {
    source: &'a str,
    env: &'a dyn Fn(&str) -> Option<String>,
}

impl Evaluator<'_> {
    fn error(&self, message: impl Into<String>) -> IncludeError {
        IncludeError::JsExpression {
            expression: self.source.to_owned(),
            message: message.into(),
        }
    }

    /// Parse the whole source into one expression.
    fn parse(&mut self) -> Result<Ast> {
        let tokens = lex(self.source);
        let mut parser = Parser {
            tokens: &tokens,
            position: 0,
            evaluator: self,
        };
        let mut mixing = Mixing::default();
        let ast = parser.ternary(&mut mixing)?;
        match parser.peek() {
            None => Ok(ast),
            Some(token) => Err(parser.unexpected(token)),
        }
    }

    /// Evaluate a parsed expression to a node.
    fn value(&self, ast: &Ast) -> Result<Node> {
        match ast {
            Ast::Literal(node) => Ok(node.clone()),
            Ast::Platform => Ok(Node::String(platform().to_owned())),
            Ast::Env(name) => Ok(match (self.env)(name) {
                Some(value) => Node::String(value),
                // `undefined`, like `null`, maps to the null node — the
                // value `??` triggers on.
                None => Node::Null,
            }),
            Ast::Cwd => match std::env::current_dir() {
                Ok(dir) => Ok(Node::String(dir.to_string_lossy().into_owned())),
                Err(error) => Err(self.error(format!("process.cwd() failed: {error}"))),
            },
            Ast::Not(inner) => Ok(Node::Bool(!truthy(&self.value(inner)?))),
            Ast::Coalesce(left, right) => {
                let left = self.value(left)?;
                if left.is_null() {
                    self.value(right)
                } else {
                    Ok(left)
                }
            }
            Ast::Or(left, right) => {
                let left = self.value(left)?;
                if truthy(&left) {
                    Ok(left)
                } else {
                    self.value(right)
                }
            }
            Ast::And(left, right) => {
                let left = self.value(left)?;
                if truthy(&left) {
                    self.value(right)
                } else {
                    Ok(left)
                }
            }
            Ast::StrictEq {
                left,
                right,
                negated,
            } => {
                let equal = strict_eq(&self.value(left)?, &self.value(right)?);
                Ok(Node::Bool(if *negated { !equal } else { equal }))
            }
            Ast::Ternary {
                condition,
                then,
                alternative,
            } => {
                if truthy(&self.value(condition)?) {
                    self.value(then)
                } else {
                    self.value(alternative)
                }
            }
        }
    }
}

/// Node's platform name, matching `process.platform` in JavaScript.
fn platform() -> &'static str {
    match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    }
}

/// JavaScript truthiness over the value domain: `null`, booleans, zero
/// numbers, and empty strings are falsy.
fn truthy(node: &Node) -> bool {
    match node {
        Node::Null => false,
        Node::Bool(value) => *value,
        Node::Int(value) => *value != 0,
        Node::UInt(value) => *value != 0,
        Node::Float(value) => *value != 0.0 && !value.is_nan(),
        Node::String(value) => !value.is_empty(),
        Node::Expr(_) | Node::Array(_) | Node::Object(_) => true,
    }
}

/// JavaScript `===` over the value domain: numbers compare numerically
/// across the tree's Int/Float split, everything else only within its
/// own kind.
fn strict_eq(left: &Node, right: &Node) -> bool {
    let numeric = |node: &Node| match node {
        Node::Int(value) => Some(*value as f64),
        Node::UInt(value) => Some(*value as f64),
        Node::Float(value) => Some(*value),
        _ => None,
    };
    match (numeric(left), numeric(right)) {
        (Some(left), Some(right)) => left == right,
        (None, None) => left == right,
        _ => false,
    }
}

// --------------------------------------------------------------- parser ---

/// Cursor over the token slice; errors carry the evaluator's source.
struct Parser<'a, 'b> {
    tokens: &'a [Token],
    position: usize,
    evaluator: &'b Evaluator<'a>,
}

impl Parser<'_, '_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.position)
    }

    /// Consume `punct` when it is next, reporting whether it was.
    fn eat(&mut self, punct: &str) -> bool {
        if matches!(self.peek(), Some(Token::Punct(found)) if *found == punct) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    /// Consume `punct` or fail with a `found`-aware message.
    fn expect(&mut self, punct: &str) -> Result<()> {
        if self.eat(punct) {
            Ok(())
        } else {
            Err(self
                .evaluator
                .error(format!("expected `{punct}`{}", self.found_suffix())))
        }
    }

    /// Consume one identifier, or fail.
    fn expect_ident(&mut self) -> Result<String> {
        match self.peek() {
            Some(Token::Ident(name)) => {
                let name = name.clone();
                self.position += 1;
                Ok(name)
            }
            _ => Err(self
                .evaluator
                .error(format!("expected a name{}", self.found_suffix()))),
        }
    }

    /// ``, found `??`'' style context for error messages.
    fn found_suffix(&self) -> String {
        match self.peek() {
            Some(token) => format!(", found {}", describe(token)),
            None => String::new(),
        }
    }

    fn unexpected(&self, token: &Token) -> IncludeError {
        match token {
            Token::LooseEq(op) => self.evaluator.error(format!(
                "loose equality `{op}` is outside the supported expression subset (use {})",
                if *op == "==" { "`===`" } else { "`!==`" }
            )),
            Token::Unsupported(text) => self.evaluator.error(format!(
                "`{text}` is outside the supported expression subset"
            )),
            other => self
                .evaluator
                .error(format!("unexpected {}", describe(other))),
        }
    }

    /// `cond ? then : alternative` — the lowest precedence, right-
    /// associative in both branches.
    fn ternary(&mut self, mixing: &mut Mixing) -> Result<Ast> {
        let condition = self.logical(mixing)?;
        if !self.eat("?") {
            return Ok(condition);
        }
        // The branches are fresh mixing regions, like parenthesized
        // subexpressions.
        let then = self.ternary(&mut Mixing::default())?;
        self.expect(":")?;
        let alternative = self.ternary(&mut Mixing::default())?;
        Ok(Ast::Ternary {
            condition: Box::new(condition),
            then: Box::new(then),
            alternative: Box::new(alternative),
        })
    }

    /// `??` / `||` (same precedence, left-associative). JavaScript makes
    /// mixing `??` with `||`/`&&` in one region a syntax error; so does
    /// the subset.
    fn logical(&mut self, mixing: &mut Mixing) -> Result<Ast> {
        let mut left = self.and_(mixing)?;
        loop {
            if self.eat("??") {
                mixing.saw_coalesce = true;
                left = Ast::Coalesce(Box::new(left), Box::new(self.and_(mixing)?));
            } else if self.eat("||") {
                mixing.saw_logical = true;
                left = Ast::Or(Box::new(left), Box::new(self.and_(mixing)?));
            } else {
                if mixing.saw_coalesce && mixing.saw_logical {
                    return Err(self.evaluator.error(
                        "cannot mix `??` with `||`/`&&` without parentheses (JavaScript syntax error)",
                    ));
                }
                return Ok(left);
            }
        }
    }

    /// `&&`, above `||`/`??` like JavaScript.
    fn and_(&mut self, mixing: &mut Mixing) -> Result<Ast> {
        let mut left = self.equality()?;
        while self.eat("&&") {
            mixing.saw_logical = true;
            left = Ast::And(Box::new(left), Box::new(self.equality()?));
        }
        Ok(left)
    }

    /// `===` / `!==`.
    fn equality(&mut self) -> Result<Ast> {
        let mut left = self.unary()?;
        loop {
            let negated = if self.eat("===") {
                false
            } else if self.eat("!==") {
                true
            } else {
                return Ok(left);
            };
            left = Ast::StrictEq {
                left: Box::new(left),
                right: Box::new(self.unary()?),
                negated,
            };
        }
    }

    /// Unary `!`.
    fn unary(&mut self) -> Result<Ast> {
        if self.eat("!") {
            return Ok(Ast::Not(Box::new(self.unary()?)));
        }
        self.primary()
    }

    /// Literals, parenthesized expressions, and `process.*` references.
    fn primary(&mut self) -> Result<Ast> {
        let Some(token) = self.peek().cloned() else {
            return Err(self.evaluator.error("unexpected end of expression"));
        };
        self.position += 1;
        match token {
            Token::Punct("(") => {
                let inner = self.ternary(&mut Mixing::default())?;
                self.expect(")")?;
                Ok(inner)
            }
            Token::Str(text) => Ok(Ast::Literal(Node::String(text))),
            Token::Int(value) => Ok(Ast::Literal(Node::Int(value))),
            Token::Float(value) => Ok(Ast::Literal(Node::Float(value))),
            Token::Ident(name) => self.ident(name),
            other => Err(self.unexpected(&other)),
        }
    }

    /// Keywords, then the `process.*` member subset; every other
    /// identifier is outside the subset (injected-context expressions
    /// evaluate lazily in the owning plugin, not here).
    fn ident(&mut self, name: String) -> Result<Ast> {
        match name.as_str() {
            "true" => Ok(Ast::Literal(Node::Bool(true))),
            "false" => Ok(Ast::Literal(Node::Bool(false))),
            "null" | "undefined" => Ok(Ast::Literal(Node::Null)),
            "process" => self.process_member(),
            other => Err(self.evaluator.error(format!(
                "`{other}` is outside the supported expression subset: at config hand-off only \
                 `process.platform`, `process.env.NAME`, and `process.cwd()` are available \
                 (injected-context expressions such as `ctx.*` or `dshHomePath(…)` evaluate \
                 lazily in the owning plugin)"
            ))),
        }
    }

    /// `process.platform` / `process.env.NAME` / `process.cwd()` — the
    /// only member chains the subset carries.
    fn process_member(&mut self) -> Result<Ast> {
        self.expect(".")?;
        let member = self.expect_ident()?;
        match member.as_str() {
            "platform" => Ok(Ast::Platform),
            "cwd" => {
                self.expect("(")?;
                self.expect(")")?;
                Ok(Ast::Cwd)
            }
            "env" => {
                self.expect(".")?;
                let name = self.expect_ident()?;
                Ok(Ast::Env(name))
            }
            other => Err(self.evaluator.error(format!(
                "`process.{other}` is outside the supported expression subset"
            ))),
        }
    }
}

/// A token's name for error messages.
fn describe(token: &Token) -> String {
    match token {
        Token::Punct(punct) => format!("`{punct}`"),
        Token::Str(_) => "a string literal".to_owned(),
        Token::Int(_) => "an integer".to_owned(),
        Token::Float(_) => "a float".to_owned(),
        Token::Ident(name) => format!("`{name}`"),
        Token::LooseEq(op) => format!("`{op}`"),
        Token::Unsupported(text) => format!("`{text}`"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An environment lookup over fixed pairs.
    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let pairs = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect::<Vec<_>>();
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        }
    }

    fn eval(source: &str, pairs: &[(&str, &str)]) -> Node {
        evaluate_with(source, &env(pairs)).expect("evaluation")
    }

    fn error(source: &str) -> String {
        evaluate_with(source, &|_| None)
            .expect_err("evaluation must fail")
            .to_string()
    }

    // --- the shipped environment-conditioned samples, one by one ---

    #[test]
    fn bare_env_fetch_yields_the_string_or_null() {
        assert_eq!(
            eval(
                "process.env.DSH_TOOLS_MODE",
                &[("DSH_TOOLS_MODE", "bundled")]
            ),
            Node::String("bundled".to_owned())
        );
        assert_eq!(eval("process.env.DSH_TOOLS_MODE", &[]), Node::Null);
    }

    #[test]
    fn platform_comparisons_follow_the_host() {
        let expected = std::env::consts::OS == "windows";
        assert_eq!(
            eval("process.platform === 'win32'", &[]),
            Node::Bool(expected)
        );
        assert_eq!(
            eval("process.platform !== 'win32'", &[]),
            Node::Bool(!expected)
        );
        assert_eq!(
            eval("process.platform", &[]),
            Node::String(platform().to_owned())
        );
    }

    #[test]
    fn cwd_is_the_process_working_directory() {
        let expected = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(eval("process.cwd()", &[]), Node::String(expected));
    }

    #[test]
    fn coalesce_falls_back_on_unset_only() {
        let url = "https://otlp.invalid/v1/logs";
        let source = "process.env.DSH_TELEMETRY_OTLP_URL ?? 'https://otlp.invalid/v1/logs'";
        assert_eq!(
            eval(source, &[("DSH_TELEMETRY_OTLP_URL", url)]),
            Node::String(url.to_owned())
        );
        assert_eq!(eval(source, &[]), Node::String(url.to_owned()));
        // `??` triggers on null/undefined only — an empty string passes.
        assert_eq!(
            eval("process.env.X ?? 'fallback'", &[("X", "")]),
            Node::String(String::new())
        );
    }

    #[test]
    fn or_falls_back_on_all_falsy_values() {
        assert_eq!(
            eval("process.env.DSH_TELEMETRY_MODE || 'DISABLED'", &[]),
            Node::String("DISABLED".to_owned())
        );
        assert_eq!(
            eval(
                "process.env.DSH_TELEMETRY_MODE || 'DISABLED'",
                &[("DSH_TELEMETRY_MODE", "")]
            ),
            Node::String("DISABLED".to_owned())
        );
        assert_eq!(
            eval(
                "process.env.DSH_TELEMETRY_MODE || 'DISABLED'",
                &[("DSH_TELEMETRY_MODE", "full")]
            ),
            Node::String("full".to_owned())
        );
    }

    #[test]
    fn permission_mode_sample_through_the_ternary() {
        let source = "(process.env.DSH_PERMISSION_MODE ?? 'workspace-write') === 'danger-full-access' ? 'never' : 'ask'";
        assert_eq!(eval(source, &[]), Node::String("ask".to_owned()));
        assert_eq!(
            eval(source, &[("DSH_PERMISSION_MODE", "danger-full-access")]),
            Node::String("never".to_owned())
        );
        // The coalesce fallback feeds the comparison.
        assert_eq!(
            eval(source, &[("DSH_PERMISSION_MODE", "workspace-write")]),
            Node::String("ask".to_owned())
        );
    }

    // --- operator semantics ---

    #[test]
    fn numbers_evaluate_across_the_int_float_split() {
        assert_eq!(eval("1 === 1.0", &[]), Node::Bool(true));
        assert_eq!(eval("'1' === 1", &[]), Node::Bool(false));
        assert_eq!(eval("null === undefined", &[]), Node::Bool(true));
        assert_eq!(eval("3080", &[]), Node::Int(3080));
        assert_eq!(eval("1.5", &[]), Node::Float(1.5));
        assert_eq!(eval("1e3", &[]), Node::Float(1000.0));
    }

    #[test]
    fn unary_not_uses_javascript_truthiness() {
        assert_eq!(eval("!''", &[]), Node::Bool(true));
        assert_eq!(eval("!0", &[]), Node::Bool(true));
        assert_eq!(eval("!null", &[]), Node::Bool(true));
        assert_eq!(eval("!undefined", &[]), Node::Bool(true));
        assert_eq!(eval("!'x'", &[]), Node::Bool(false));
        assert_eq!(eval("!process.env.MISSING", &[]), Node::Bool(true));
    }

    #[test]
    fn logical_operators_keep_value_semantics() {
        assert_eq!(eval("false && 'x'", &[]), Node::Bool(false));
        assert_eq!(eval("'' && 'x'", &[]), Node::String(String::new()));
        assert_eq!(eval("true && 'x'", &[]), Node::String("x".to_owned()));
        assert_eq!(eval("'a' || 'b'", &[]), Node::String("a".to_owned()));
        // && binds tighter than ||, like JavaScript.
        assert_eq!(
            eval("false || 'yes' && 'no'", &[]),
            Node::String("no".to_owned())
        );
    }

    #[test]
    fn nested_ternaries_and_parens() {
        assert_eq!(
            eval("true ? false ? 'a' : 'b' : 'c'", &[]),
            Node::String("b".to_owned())
        );
        assert_eq!(
            eval("(true ? false : true) ? 'a' : 'b'", &[]),
            Node::String("b".to_owned())
        );
    }

    #[test]
    fn double_quoted_strings_and_escapes() {
        assert_eq!(
            eval(r#""double 'quoted'""#, &[]),
            Node::String("double 'quoted'".to_owned())
        );
        assert_eq!(
            eval(r"'line\nbreak'", &[]),
            Node::String("line\nbreak".to_owned())
        );
        assert_eq!(
            eval(r#""tab\there""#, &[]),
            Node::String("tab\there".to_owned())
        );
    }

    #[test]
    fn coalesce_may_not_mix_with_logical_operators() {
        // JavaScript rejects these without parentheses; so does the subset.
        assert!(error("process.env.X ?? 'a' || 'b'").contains("mix"));
        assert!(error("process.env.X ?? 'a' && 'b'").contains("mix"));
        assert!(error("true || false ?? null").contains("mix"));
        // Parenthesized regions are separate and fine.
        assert_eq!(
            eval("(process.env.X ?? 'a') || 'b'", &[]),
            Node::String("a".to_owned())
        );
    }

    // --- out-of-subset references fail with a clear message ---

    #[test]
    fn injected_context_references_are_outside_the_subset() {
        for source in [
            "ctx.webStartup.trustedHosts",
            "ctx.webRuntime.trustedHosts",
            "ctx.headlessStartup.task",
            "ctx.webStartup.port ?? 3080",
            "ctx.webStartup.host ?? '127.0.0.1'",
            "dshHomePath('storages')",
            "dshHomePath('sessions')",
        ] {
            let message = error(source);
            assert!(message.contains("subset"), "{source}: {message}");
            assert!(message.contains("process.cwd"), "{source}: {message}");
        }
    }

    #[test]
    fn other_javascript_is_outside_the_subset() {
        assert!(error("process.foo").contains("subset"));
        assert!(error("process.env").contains("expected"));
        assert!(error("process.cwd").contains("expected"));
        assert!(error("1 == 1").contains("loose equality"));
        assert!(error("1 != 1").contains("loose equality"));
        assert!(error("'a' + 'b'").contains("subset"));
        assert!(error("1 < 2").contains("subset"));
        assert!(error("typeof 'x'").contains("subset"));
        assert!(error("env.HOME").contains("subset"));
    }

    #[test]
    fn syntax_errors_are_reported() {
        assert!(error("'unterminated").contains("subset"));
        assert!(error("process.platform ===").contains("unexpected end"));
        assert!(error("true ? 'a'").contains("expected `:`"));
        assert!(error("(true").contains("expected `)`"));
        assert!(error("true false").contains("unexpected"));
    }

    #[test]
    fn evaluate_reads_the_process_environment() {
        // A name nothing sets: `undefined`, which `??` replaces.
        let source = "process.env.CORDIS_EXPR_TEST_UNSET_7f3a ?? 'fallback'";
        assert_eq!(
            evaluate(source).unwrap(),
            Node::String("fallback".to_owned())
        );
        // The working directory through the public entry point.
        let cwd = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(evaluate("process.cwd()").unwrap(), Node::String(cwd));
    }

    #[test]
    fn evaluate_node_recurses_the_tree() {
        let node = Node::from_iter([
            ("mode".to_owned(), Node::Expr("process.env.MODE".to_owned())),
            (
                "nested".to_owned(),
                Node::Array(vec![
                    Node::Expr("process.platform === 'win32'".to_owned()),
                    Node::String("kept".to_owned()),
                ]),
            ),
        ]);
        let evaluated =
            evaluate_node_with(&node, &|name| (name == "MODE").then(|| "fast".to_owned())).unwrap();
        let map = evaluated.as_object().unwrap();
        assert_eq!(map["mode"], Node::String("fast".to_owned()));
        assert_eq!(
            map["nested"].as_array().unwrap()[0],
            Node::Bool(std::env::consts::OS == "windows")
        );
        assert_eq!(
            map["nested"].as_array().unwrap()[1],
            Node::String("kept".to_owned())
        );
    }
}
