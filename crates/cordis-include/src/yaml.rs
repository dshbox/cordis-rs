//! The entry-list YAML dialect: tag-preserving parse and emit.
//!
//! Upstream's config and patch files are written in a js-yaml dialect where
//! `!!js` scalars round-trip as expression nodes the loader evaluates at
//! entry activation. `serde_yaml_ng` silently drops non-standard tags (a
//! `!!js` scalar arrives as its plain text), so this module drives
//! `unsafe-libyaml`'s parser directly — the same event stream serde_yaml_ng
//! consumes internally, built into [`Node`] with `!!js` kept as
//! [`Node::Expr`] — and pairs it with a hand-written emitter that prints
//! expressions back as `!!js <expr>`.
//!
//! The dialect matches what the serde path produced before the switch:
//! scalar resolution (null/bool/int/float by content, YAML 1.2 core-ish,
//! leading-zero digit runs staying strings), quoted scalars always strings,
//! local `!tags` on plain scalars dropped, anchors and aliases resolved,
//! and a single document per input. The gain: `!!js` survives, and syntax
//! errors carry line and column.
//!
//! The `unsafe-libyaml` driving layer is the only unsafe code in this
//! crate, item-scoped in the private `sys` module with SAFETY notes (the
//! same pattern `cordis-loader` uses for libloading).

use crate::Document;
use crate::error::{IncludeError, Result};
use crate::node::{Node, NodeMap};
use crate::options::EntryOptions;
use crate::patch::PatchOptions;

/// The full tag URI js-yaml's `!!js` resolves to.
const JS_TAG: &str = "tag:yaml.org,2002:js";

/// Core YAML tags (`!!bool`, `!!int`, `!!float`, `!!null`, `!!str`).
fn core_tag(tag: &str) -> Option<&'static str> {
    const CORE: [&str; 5] = ["bool", "int", "float", "null", "str"];
    let local = tag.strip_prefix("tag:yaml.org,2002:")?;
    CORE.into_iter().find(|candidate| *candidate == local)
}

/// A dialect failure with location context when the parser produced one.
#[derive(Debug)]
struct DialectError {
    message: String,
}

impl std::fmt::Display for DialectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DialectError {}

fn yaml_error(message: impl Into<String>) -> IncludeError {
    IncludeError::Parse {
        format: "yaml",
        source: Box::new(DialectError {
            message: message.into(),
        }),
    }
}

// ------------------------------------------------------------------ sys ---

/// Safe surface over `unsafe-libyaml`'s parser: one event per call with
/// decoded anchor/tag/value strings and the event's start mark.
mod sys {
    #![allow(unsafe_code)]
    // SAFETY notes in this module refer to `unsafe-libyaml`'s C-translation
    // contract: event structs are owned, `yaml_parser_parse` writes exactly
    // one event per successful call, and every produced event must be
    // deleted. The parser is kept at a stable heap address for its lifetime.
    // `sys` is the conventional alias; the whole module is the documented
    // unsafe boundary, so the name not repeating "unsafe" loses nothing.
    #[allow(clippy::unsafe_removed_from_name)]
    use unsafe_libyaml as sys;

    /// One parser event with its data decoded to owned strings.
    pub(super) struct Event {
        pub kind: EventKind,
        pub anchor: Option<String>,
        pub tag: Option<String>,
        pub value: String,
        pub style: ScalarStyle,
        /// Zero-based start position of the event.
        pub line: u64,
        pub column: u64,
    }

    pub(super) enum EventKind {
        StreamStart,
        StreamEnd,
        DocumentStart,
        DocumentEnd,
        Alias,
        Scalar,
        SequenceStart,
        SequenceEnd,
        MappingStart,
        MappingEnd,
    }

    pub(super) enum ScalarStyle {
        Plain,
        Quoted,
    }

    /// A parser failure with the problem and its zero-based location.
    pub(super) struct Error {
        pub problem: String,
        pub line: u64,
        pub column: u64,
    }

    pub(super) struct Parser {
        raw: std::boxed::Box<std::mem::MaybeUninit<sys::yaml_parser_t>>,
    }

    impl Parser {
        pub(super) fn new(input: &[u8]) -> Parser {
            let mut raw = std::boxed::Box::new(std::mem::MaybeUninit::uninit());
            // SAFETY: `raw` is heap-stable for the parser's lifetime (never
            // moved out), and `yaml_parser_initialize` fully initializes
            // every field of a valid `yaml_parser_t`.
            unsafe {
                let parser = raw.as_mut_ptr();
                if sys::yaml_parser_initialize(parser).fail {
                    panic!("libyaml parser allocation failed");
                }
                sys::yaml_parser_set_encoding(parser, sys::YAML_UTF8_ENCODING);
                sys::yaml_parser_set_input_string(parser, input.as_ptr(), input.len() as u64);
            }
            Parser { raw }
        }

        pub(super) fn next(&mut self) -> std::result::Result<Event, Error> {
            let mut event = std::mem::MaybeUninit::<sys::yaml_event_t>::uninit();
            // SAFETY: the parser pointer stays valid (stable box), and the
            // event out-pointer receives exactly one initialized event on
            // success. The event is converted and deleted before return so
            // its owned buffers never leak.
            unsafe {
                let parser = self.raw.as_mut_ptr();
                if sys::yaml_parser_parse(parser, event.as_mut_ptr()).fail {
                    return Err(Self::error(parser));
                }
                let converted = convert_event(&*event.as_ptr());
                sys::yaml_event_delete(event.as_mut_ptr());
                Ok(converted)
            }
        }

        /// SAFETY: `parser` points to an initialized parser that just
        /// reported a failure; the problem/context fields are C strings or
        /// null. The explicit dereference below creates the one shared
        /// reference every field read goes through — no implicit autoref
        /// ever forms through the raw pointer.
        unsafe fn error(parser: *mut sys::yaml_parser_t) -> Error {
            unsafe {
                let parser: &sys::yaml_parser_t = &*parser;
                let problem = cstring(std::ptr::addr_of!(parser.problem).cast())
                    .unwrap_or_else(|| "libyaml parser failed".to_owned());
                let mark = std::ptr::addr_of!(parser.problem_mark).read();
                Error {
                    problem,
                    line: mark.line,
                    column: mark.column,
                }
            }
        }
    }

    impl Drop for Parser {
        fn drop(&mut self) {
            // SAFETY: the box holds an initialized parser (see `new`).
            // `yaml_parser_t` is a plain C struct (no destructor to run);
            // the delete call frees its internal buffers and the box frees
            // the storage.
            unsafe {
                sys::yaml_parser_delete(self.raw.as_mut_ptr());
            }
        }
    }

    /// SAFETY: `ptr` is null or a valid C string.
    unsafe fn cstring(ptr: *const u8) -> Option<String> {
        unsafe {
            if ptr.is_null() {
                return None;
            }
            let mut length = 0usize;
            while *ptr.add(length) != 0 {
                length += 1;
            }
            let bytes = std::slice::from_raw_parts(ptr, length);
            Some(String::from_utf8_lossy(bytes).into_owned())
        }
    }

    /// SAFETY: `event` was produced by a successful `yaml_parser_parse`
    /// call and has not been deleted yet.
    unsafe fn convert_event(event: &sys::yaml_event_t) -> Event {
        let mark = event.start_mark;
        let base = |kind| Event {
            kind,
            anchor: None,
            tag: None,
            value: String::new(),
            style: ScalarStyle::Plain,
            line: mark.line,
            column: mark.column,
        };
        // SAFETY: reading the union member that matches `type_` is the
        // contract of `yaml_event_t`; scalar buffers are `length` bytes.
        unsafe {
            match event.type_ {
                sys::YAML_STREAM_START_EVENT => base(EventKind::StreamStart),
                sys::YAML_STREAM_END_EVENT => base(EventKind::StreamEnd),
                sys::YAML_DOCUMENT_START_EVENT => base(EventKind::DocumentStart),
                sys::YAML_DOCUMENT_END_EVENT => base(EventKind::DocumentEnd),
                sys::YAML_ALIAS_EVENT => Event {
                    anchor: cstring(event.data.alias.anchor),
                    ..base(EventKind::Alias)
                },
                sys::YAML_SCALAR_EVENT => {
                    let length = event.data.scalar.length as usize;
                    let bytes = if event.data.scalar.value.is_null() {
                        &[][..]
                    } else {
                        std::slice::from_raw_parts(event.data.scalar.value, length)
                    };
                    Event {
                        kind: EventKind::Scalar,
                        anchor: cstring(event.data.scalar.anchor),
                        tag: cstring(event.data.scalar.tag),
                        value: String::from_utf8_lossy(bytes).into_owned(),
                        style: match event.data.scalar.style {
                            sys::YAML_PLAIN_SCALAR_STYLE => ScalarStyle::Plain,
                            _ => ScalarStyle::Quoted,
                        },
                        line: mark.line,
                        column: mark.column,
                    }
                }
                sys::YAML_SEQUENCE_START_EVENT => Event {
                    anchor: cstring(event.data.sequence_start.anchor),
                    tag: cstring(event.data.sequence_start.tag),
                    ..base(EventKind::SequenceStart)
                },
                sys::YAML_SEQUENCE_END_EVENT => base(EventKind::SequenceEnd),
                sys::YAML_MAPPING_START_EVENT => Event {
                    anchor: cstring(event.data.mapping_start.anchor),
                    tag: cstring(event.data.mapping_start.tag),
                    ..base(EventKind::MappingStart)
                },
                sys::YAML_MAPPING_END_EVENT => base(EventKind::MappingEnd),
                _ => unreachable!("libyaml produced an event outside the parser state machine"),
            }
        }
    }
}

// ---------------------------------------------------------------- parse ---

/// Parse one YAML document into a [`Node`] tree, preserving `!!js`
/// scalars as [`Node::Expr`]. An empty input yields [`Node::Null`].
///
/// # Errors
///
/// Syntax errors carry the parser's line and column; more than one
/// document and unresolved aliases are rejected.
pub fn parse_node(source: &str) -> Result<Node> {
    let mut parser = sys::Parser::new(source.as_bytes());
    let mut anchors: Vec<(String, Node)> = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut root: Option<Node> = None;
    let mut documents = 0usize;

    loop {
        let event = parser.next().map_err(|error| {
            yaml_error(format!(
                "{} at line {} column {}",
                error.problem,
                error.line + 1,
                error.column + 1
            ))
        })?;
        match event.kind {
            sys::EventKind::StreamStart | sys::EventKind::DocumentEnd => {}
            sys::EventKind::DocumentStart => {
                documents += 1;
                if documents > 1 {
                    return Err(yaml_error(
                        "deserializing from YAML containing more than one document is not supported",
                    ));
                }
            }
            sys::EventKind::StreamEnd => break,
            sys::EventKind::Scalar => {
                // A scalar in key position becomes its raw text — the
                // serde path's leniency for plain non-string keys.
                if let Some(Frame::Map {
                    key: slot @ None, ..
                }) = stack.last_mut()
                {
                    *slot = Some(event.value.clone());
                } else {
                    let node = resolve_scalar(&event)?;
                    register_anchor(&mut anchors, &event.anchor, &node);
                    feed(&mut stack, &mut root, node)?;
                }
            }
            sys::EventKind::Alias => {
                let Some(anchor) = &event.anchor else {
                    return Err(yaml_error("alias without an anchor"));
                };
                let Some(node) = anchors
                    .iter()
                    .rev()
                    .find(|(name, _)| name == anchor)
                    .map(|(_, node)| node.clone())
                else {
                    return Err(yaml_error(format!(
                        "unknown anchor {anchor:?} at line {} column {}",
                        event.line + 1,
                        event.column + 1
                    )));
                };
                feed(&mut stack, &mut root, node)?;
            }
            sys::EventKind::SequenceStart => {
                reject_local_tag(&event)?;
                stack.push(Frame::Seq {
                    items: Vec::new(),
                    anchor: event.anchor,
                });
            }
            sys::EventKind::SequenceEnd => {
                let Some(Frame::Seq { items, anchor }) = stack.pop() else {
                    return Err(yaml_error("unbalanced sequence end"));
                };
                let node = Node::Array(items);
                register_anchor(&mut anchors, &anchor, &node);
                feed(&mut stack, &mut root, node)?;
            }
            sys::EventKind::MappingStart => {
                reject_local_tag(&event)?;
                stack.push(Frame::Map {
                    map: NodeMap::new(),
                    key: None,
                    anchor: event.anchor,
                });
            }
            sys::EventKind::MappingEnd => {
                let Some(Frame::Map {
                    map,
                    key: None,
                    anchor,
                }) = stack.pop()
                else {
                    return Err(yaml_error("unbalanced mapping end"));
                };
                let node = Node::Object(map);
                register_anchor(&mut anchors, &anchor, &node);
                feed(&mut stack, &mut root, node)?;
            }
        }
    }
    Ok(root.unwrap_or(Node::Null))
}

/// One open container on the parse stack.
enum Frame {
    Seq {
        items: Vec<Node>,
        anchor: Option<String>,
    },
    Map {
        map: NodeMap,
        key: Option<String>,
        anchor: Option<String>,
    },
}

fn register_anchor(anchors: &mut Vec<(String, Node)>, anchor: &Option<String>, node: &Node) {
    if let Some(anchor) = anchor {
        anchors.push((anchor.clone(), node.clone()));
    }
}

/// Hand one completed node to the enclosing container (or the root).
fn feed(stack: &mut [Frame], root: &mut Option<Node>, node: Node) -> Result<()> {
    match stack.last_mut() {
        None => {
            if root.is_some() {
                return Err(yaml_error("multiple root values in one document"));
            }
            *root = Some(node);
        }
        Some(Frame::Seq { items, .. }) => items.push(node),
        Some(Frame::Map { map, key, .. }) => match key.take() {
            None => {
                return Err(yaml_error(format!(
                    "mapping keys must be scalars, found {}",
                    node_kind(&node)
                )));
            }
            Some(name) => {
                map.insert(name, node);
            }
        },
    }
    Ok(())
}

/// Local tags on containers are serde enum syntax as well.
fn reject_local_tag(event: &sys::Event) -> Result<()> {
    if event.tag.as_deref().is_some_and(|tag| tag.starts_with('!')) {
        return Err(yaml_error(format!(
            "local tags are not supported: {}",
            event.tag.as_deref().unwrap_or_default()
        )));
    }
    Ok(())
}

/// Resolve one scalar event into a [`Node`], mirroring the serde path's
/// rules except that `!!js` becomes [`Node::Expr`].
fn resolve_scalar(event: &sys::Event) -> Result<Node> {
    let value = &event.value;
    if let Some(tag) = &event.tag {
        if tag == JS_TAG {
            return Ok(Node::Expr(value.clone()));
        }
        if let Some(core) = core_tag(tag) {
            return match core {
                "bool" => parse_bool(value)
                    .map(Node::Bool)
                    .ok_or_else(|| yaml_error(format!("invalid boolean {value:?}"))),
                "int" => {
                    try_int(value)?.ok_or_else(|| yaml_error(format!("invalid integer {value:?}")))
                }
                "float" => parse_f64(value)
                    .map(Node::Float)
                    .ok_or_else(|| yaml_error(format!("invalid float {value:?}"))),
                "null" => parse_null(value)
                    .map(|()| Node::Null)
                    .ok_or_else(|| yaml_error(format!("invalid null {value:?}"))),
                "str" => Ok(Node::String(value.clone())),
                _ => unreachable!("core_tag filters its output"),
            };
        }
        if tag.starts_with('!') {
            // Local tags are serde's enum syntax; the typed paths rejected
            // them, and so does the dialect.
            return Err(yaml_error(format!("local tags are not supported: {tag}")));
        }
        // Other non-core global tags (or any tag on a quoted scalar): the
        // serde path dropped these to strings; keep that.
        return Ok(Node::String(value.clone()));
    }
    if matches!(event.style, sys::ScalarStyle::Plain) {
        resolve_plain(value)
    } else {
        Ok(Node::String(value.clone()))
    }
}

/// Resolve an untagged plain scalar by content — the exact rules the serde
/// path applied, kept byte-for-byte so files parsed before the switch
/// compose identically after it.
fn resolve_plain(value: &str) -> Result<Node> {
    if value.is_empty() || parse_null(value).is_some() {
        return Ok(Node::Null);
    }
    if let Some(boolean) = parse_bool(value) {
        return Ok(Node::Bool(boolean));
    }
    if let Some(node) = try_int(value)? {
        return Ok(node);
    }
    if !digits_but_not_number(value) {
        if let Some(float) = parse_f64(value) {
            return Ok(Node::Float(float));
        }
    }
    Ok(Node::String(value.to_owned()))
}

/// Unsigned integers within `i64` range become [`Node::Int`]; only values
/// above it stay [`Node::UInt`].
fn small_uint(value: u64) -> Node {
    if value <= i64::MAX as u64 {
        Node::Int(value as i64)
    } else {
        Node::UInt(value)
    }
}

/// Leading-zero digit runs are strings per YAML 1.2, not numbers.
fn digits_but_not_number(scalar: &str) -> bool {
    let scalar = scalar.strip_prefix(['-', '+']).unwrap_or(scalar);
    scalar.len() > 1 && scalar.starts_with('0') && scalar[1..].bytes().all(|b| b.is_ascii_digit())
}

/// The unsigned integer forms: optional `+`, `0x`/`0o`/`0b` prefixes
/// without a sign, or plain decimal.
fn parse_unsigned(scalar: &str, radix_skip: fn(&str, u32) -> Option<u64>) -> Option<u64> {
    let unpositive = scalar.strip_prefix('+').unwrap_or(scalar);
    if let Some(rest) = unpositive.strip_prefix("0x") {
        if !rest.starts_with(['+', '-']) {
            if let Some(int) = radix_skip(rest, 16) {
                return Some(int);
            }
        }
    }
    if let Some(rest) = unpositive.strip_prefix("0o") {
        if !rest.starts_with(['+', '-']) {
            if let Some(int) = radix_skip(rest, 8) {
                return Some(int);
            }
        }
    }
    if let Some(rest) = unpositive.strip_prefix("0b") {
        if !rest.starts_with(['+', '-']) {
            if let Some(int) = radix_skip(rest, 2) {
                return Some(int);
            }
        }
    }
    if unpositive.starts_with(['+', '-']) {
        return None;
    }
    if digits_but_not_number(scalar) {
        return None;
    }
    radix_skip(unpositive, 10)
}

/// The negative integer forms: `-0x`/`-0o`/`-0b` and plain negative
/// decimal; plain positive decimals also parse (callers reach this only
/// after the unsigned forms failed).
fn parse_negative(scalar: &str, radix_skip: fn(&str, u32) -> Option<i64>) -> Option<i64> {
    for prefix in ["-0x", "-0o", "-0b"] {
        if let Some(rest) = scalar.strip_prefix(prefix) {
            let radix = match prefix {
                "-0x" => 16,
                "-0o" => 8,
                _ => 2,
            };
            if let Some(int) = radix_skip(rest, radix) {
                return Some(-int);
            }
        }
    }
    if digits_but_not_number(scalar) {
        return None;
    }
    radix_skip(scalar, 10)
}

fn u64_radix(text: &str, radix: u32) -> Option<u64> {
    u64::from_str_radix(text, radix).ok()
}

fn i64_radix(text: &str, radix: u32) -> Option<i64> {
    i64::from_str_radix(text, radix).ok()
}

/// Parse an integer scalar into a node, or `None` when it is not one.
/// Integers that parse but fall outside `i64`/`u64` range are an error —
/// the tree has no slot for them (matching the serde path).
fn try_int(scalar: &str) -> Result<Option<Node>> {
    if let Some(unsigned) = parse_unsigned(scalar, u64_radix) {
        return Ok(Some(small_uint(unsigned)));
    }
    if let Some(signed) = parse_negative(scalar, i64_radix) {
        return Ok(Some(Node::Int(signed)));
    }
    // Values that parse as wider integers overflow the tree's number slots.
    if parse_unsigned(scalar, |text, radix| {
        u128::from_str_radix(text, radix).ok().map(|_| 0)
    })
    .is_some()
        || parse_negative(scalar, |text, radix| {
            i128::from_str_radix(text, radix).ok().map(|_| 0)
        })
        .is_some()
    {
        return Err(yaml_error(format!("integer out of range: {scalar:?}")));
    }
    Ok(None)
}

fn parse_null(scalar: &str) -> Option<()> {
    match scalar {
        "null" | "Null" | "NULL" | "~" => Some(()),
        _ => None,
    }
}

fn parse_bool(scalar: &str) -> Option<bool> {
    match scalar {
        "true" | "True" | "TRUE" => Some(true),
        "false" | "False" | "FALSE" => Some(false),
        _ => None,
    }
}

fn parse_f64(scalar: &str) -> Option<f64> {
    let unpositive = if let Some(unpositive) = scalar.strip_prefix('+') {
        if unpositive.starts_with(['+', '-']) {
            return None;
        }
        unpositive
    } else {
        scalar
    };
    if let ".inf" | ".Inf" | ".INF" = unpositive {
        return Some(f64::INFINITY);
    }
    if let "-.inf" | "-.Inf" | "-.INF" = scalar {
        return Some(f64::NEG_INFINITY);
    }
    if let ".nan" | ".NaN" | ".NAN" = scalar {
        return Some(f64::NAN.copysign(1.0));
    }
    if let Ok(float) = unpositive.parse::<f64>() {
        if float.is_finite() {
            return Some(float);
        }
    }
    None
}

/// A human name for a node kind, for conversion diagnostics.
fn node_kind(node: &Node) -> &'static str {
    match node {
        Node::Null => "null",
        Node::Bool(_) => "a boolean",
        Node::Int(_) | Node::UInt(_) => "an integer",
        Node::Float(_) => "a float",
        Node::String(_) => "a string",
        Node::Expr(_) => "a !!js expression",
        Node::Array(_) => "a sequence",
        Node::Object(_) => "a mapping",
    }
}

// ----------------------------------------------------------- converters ---

/// Parse one YAML document into a [`Document`]: an `entries` list plus
/// unknown top-level keys, null documents yielding the default.
pub fn parse_document(source: &str) -> Result<Document> {
    document_from_node(parse_node(source)?)
}

/// Parse one YAML document into a top-level entry list (the include/patch
/// file shape).
pub fn parse_entry_list(source: &str) -> Result<Vec<EntryOptions>> {
    entry_list_from_node(parse_node(source)?)
}

/// Convert a parsed tree into a [`Document`].
pub fn document_from_node(node: Node) -> Result<Document> {
    let Node::Object(map) = node else {
        return Err(yaml_error(format!(
            "expected a mapping with an `entries` list, found {}",
            node_kind(&node)
        )));
    };
    let mut document = Document::default();
    for (key, value) in map {
        if key == "entries" {
            // A null entries list is the field's default (an empty file
            // tail like `entries:`), matching the previous leniency.
            document.entries = match value {
                Node::Null => Vec::new(),
                other => {
                    entry_list_from_node(other).map_err(|error| prepend(error, "entries: "))?
                }
            };
        } else {
            document.extra.insert(key, value);
        }
    }
    Ok(document)
}

/// Convert a parsed tree into an entry list.
pub fn entry_list_from_node(node: Node) -> Result<Vec<EntryOptions>> {
    let Node::Array(items) = node else {
        return Err(yaml_error(format!(
            "expected a sequence of entries, found {}",
            node_kind(&node)
        )));
    };
    items
        .into_iter()
        .enumerate()
        .map(|(index, node)| {
            entry_from_node(node).map_err(|error| prepend(error, &format!("entry {}: ", index + 1)))
        })
        .collect()
}

/// Convert a parsed tree into a patch list.
pub fn patch_list_from_node(node: Node) -> Result<Vec<PatchOptions>> {
    let Node::Array(items) = node else {
        return Err(yaml_error(format!(
            "expected a sequence of patch entries, found {}",
            node_kind(&node)
        )));
    };
    items
        .into_iter()
        .enumerate()
        .map(|(index, node)| {
            patch_from_node(node).map_err(|error| prepend(error, &format!("patch {}: ", index + 1)))
        })
        .collect()
}

fn prepend(error: IncludeError, context: &str) -> IncludeError {
    let message = match &error {
        IncludeError::Parse { source, .. } => source.to_string(),
        other => other.to_string(),
    };
    yaml_error(format!("{context}{message}"))
}

/// Convert one entry mapping.
fn entry_from_node(node: Node) -> Result<EntryOptions> {
    let Node::Object(map) = node else {
        return Err(yaml_error(format!(
            "expected a mapping (an entry), found {}",
            node_kind(&node)
        )));
    };
    let mut entry = EntryOptions::default();
    for (key, value) in map {
        match key.as_str() {
            "id" => entry.id = optional_string(value, "id")?,
            "name" => {
                entry.name = match value {
                    // A null name is the field's default (`name:` tails).
                    Node::Null => String::new(),
                    other => string_value(other, "name")?,
                };
            }
            "disabled" => {
                entry.disabled = match value {
                    Node::Bool(flag) => flag,
                    Node::Expr(_) => {
                        return Err(yaml_error(
                            "disabled: !!js expressions are not supported yet",
                        ));
                    }
                    other => {
                        return Err(yaml_error(format!(
                            "disabled: expected a boolean, found {}",
                            node_kind(&other)
                        )));
                    }
                };
            }
            // Null defaults for list fields: `inject:` / `group:` tails.
            "inject" => {
                entry.inject = match value {
                    Node::Null => Vec::new(),
                    other => string_list(other, "inject")?,
                };
            }
            "group" => {
                entry.group = match value {
                    Node::Null => Vec::new(),
                    other => {
                        entry_list_from_node(other).map_err(|error| prepend(error, "group: "))?
                    }
                };
            }
            "config" => entry.config = optional_config(value),
            // Unknown entry keys are dropped, matching the serde path.
            _ => {}
        }
    }
    Ok(entry)
}

/// Convert one patch mapping; every key without a typed slot lands in
/// `extra`, matching serde's flatten.
pub(crate) fn patch_from_node(node: Node) -> Result<PatchOptions> {
    let Node::Object(map) = node else {
        return Err(yaml_error(format!(
            "expected a mapping (a loader patch entry), found {}",
            node_kind(&node)
        )));
    };
    let mut patch = PatchOptions::default();
    for (key, value) in map {
        match key.as_str() {
            "id" => patch.id = optional_string(value, "id")?,
            "insert" => {
                patch.insert = match value {
                    Node::Null => None,
                    Node::Array(_) => Some(
                        entry_list_from_node(value).map_err(|error| prepend(error, "insert: "))?,
                    ),
                    other => {
                        return Err(yaml_error(format!(
                            "insert: expected a sequence of entries, found {}",
                            node_kind(&other)
                        )));
                    }
                };
            }
            "name" => patch.name = optional_string(value, "name")?,
            "config" => patch.config = optional_config(value),
            "disabled" => {
                patch.disabled = match value {
                    Node::Null => None,
                    Node::Bool(flag) => Some(flag),
                    Node::Expr(_) => {
                        return Err(yaml_error(
                            "disabled: !!js expressions are not supported yet",
                        ));
                    }
                    other => {
                        return Err(yaml_error(format!(
                            "disabled: expected a boolean, found {}",
                            node_kind(&other)
                        )));
                    }
                };
            }
            "inject" => {
                patch.inject = match value {
                    Node::Null => None,
                    Node::Array(_) => Some(string_list(value, "inject")?),
                    other => {
                        return Err(yaml_error(format!(
                            "inject: expected a list of strings, found {}",
                            node_kind(&other)
                        )));
                    }
                };
            }
            other => {
                patch.extra.insert(other.to_owned(), value);
            }
        }
    }
    Ok(patch)
}

fn optional_string(node: Node, field: &str) -> Result<Option<String>> {
    match node {
        Node::Null => Ok(None),
        Node::String(value) => Ok(Some(value)),
        other => Err(yaml_error(format!(
            "{field}: expected a string, found {}",
            node_kind(&other)
        ))),
    }
}

fn string_value(node: Node, field: &str) -> Result<String> {
    match node {
        Node::String(value) => Ok(value),
        other => Err(yaml_error(format!(
            "{field}: expected a string, found {}",
            node_kind(&other)
        ))),
    }
}

fn string_list(node: Node, field: &str) -> Result<Vec<String>> {
    let Node::Array(items) = node else {
        return Err(yaml_error(format!(
            "{field}: expected a list of strings, found {}",
            node_kind(&node)
        )));
    };
    items
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            string_value(item, &format!("{field}[{}]", index))
                .map_err(|error| prepend(error, &format!("{field}: entry {}: ", index + 1)))
        })
        .collect()
}

/// Config is any node; a null config means "absent".
fn optional_config(node: Node) -> Option<Node> {
    match node {
        Node::Null => None,
        other => Some(other),
    }
}

// ---------------------------------------------------------------- emit ---

/// Render a document as YAML in the crate's stable layout: `entries`
/// first (omitted when empty), then unknown top-level keys.
pub fn emit_document(document: &Document) -> String {
    let mut map = NodeMap::new();
    if !document.entries.is_empty() {
        map.insert("entries".to_owned(), entries_to_node(&document.entries));
    }
    for (key, value) in &document.extra {
        map.insert(key.clone(), value.clone());
    }
    let mut out = String::new();
    if map.is_empty() {
        out.push_str("{}\n");
    } else {
        emit_map(&map, 0, "", &mut out);
    }
    out
}

/// Render a top-level entry list as YAML (the include/patch file shape).
pub fn emit_entry_list(entries: &[EntryOptions]) -> String {
    let mut out = String::new();
    if entries.is_empty() {
        out.push_str("[]\n");
    } else {
        emit_node(&entries_to_node(entries), 0, "", &mut out);
    }
    out
}

/// Entries as a node tree, in the crate's stable field order
/// (`id`, `name`, `disabled`, `inject`, `group`, `config`) with default
/// fields omitted — the same shape the serde writer produced.
fn entries_to_node(entries: &[EntryOptions]) -> Node {
    Node::Array(entries.iter().map(entry_to_node).collect())
}

fn entry_to_node(entry: &EntryOptions) -> Node {
    let mut map = NodeMap::new();
    if let Some(id) = &entry.id {
        map.insert("id".to_owned(), Node::String(id.clone()));
    }
    map.insert("name".to_owned(), Node::String(entry.name.clone()));
    if entry.disabled {
        map.insert("disabled".to_owned(), Node::Bool(true));
    }
    if !entry.inject.is_empty() {
        map.insert(
            "inject".to_owned(),
            Node::Array(
                entry
                    .inject
                    .iter()
                    .map(|name| Node::String(name.clone()))
                    .collect(),
            ),
        );
    }
    if !entry.group.is_empty() {
        map.insert("group".to_owned(), entries_to_node(&entry.group));
    }
    if let Some(config) = &entry.config {
        map.insert("config".to_owned(), config.clone());
    }
    Node::Object(map)
}

fn spaces(indent: usize) -> String {
    " ".repeat(indent)
}

/// Emit one node at `indent`; the first line starts with `first_lead`
/// (spaces, or `"- "` when the node opens a sequence item).
fn emit_node(node: &Node, indent: usize, first_lead: &str, out: &mut String) {
    match node {
        Node::Array(items) => emit_seq(items, indent, first_lead, out),
        Node::Object(map) => emit_map(map, indent, first_lead, out),
        scalar => {
            out.push_str(first_lead);
            out.push_str(&scalar_text(scalar));
            out.push('\n');
        }
    }
}

fn emit_map(map: &NodeMap, indent: usize, first_lead: &str, out: &mut String) {
    let indented = spaces(indent);
    for (position, (key, value)) in map.iter().enumerate() {
        let lead: &str = if position == 0 { first_lead } else { &indented };
        let key_text = scalar_text(&Node::String(key.clone()));
        match value {
            Node::Array(items) if items.is_empty() => {
                out.push_str(&format!("{lead}{key_text}: []\n"));
            }
            Node::Object(inner) if inner.is_empty() => {
                out.push_str(&format!("{lead}{key_text}: {{}}\n"));
            }
            Node::Object(inner) => {
                out.push_str(&format!("{lead}{key_text}:\n"));
                emit_map(inner, indent + 2, &spaces(indent + 2), out);
            }
            // Block sequences under a map key start at the key's own
            // indent — the layout libyaml produces.
            Node::Array(items) => {
                out.push_str(&format!("{lead}{key_text}:\n"));
                emit_seq(items, indent, &spaces(indent), out);
            }
            scalar => {
                out.push_str(&format!("{lead}{key_text}: {}\n", scalar_text(scalar)));
            }
        }
    }
}

fn emit_seq(items: &[Node], indent: usize, first_lead: &str, out: &mut String) {
    let indented = spaces(indent);
    for (position, item) in items.iter().enumerate() {
        let lead: &str = if position == 0 { first_lead } else { &indented };
        match item {
            Node::Object(map) if map.is_empty() => {
                out.push_str(&format!("{lead}- {{}}\n"));
            }
            Node::Object(map) => emit_map(map, indent + 2, &format!("{lead}- "), out),
            Node::Array(inner) if inner.is_empty() => {
                out.push_str(&format!("{lead}- []\n"));
            }
            Node::Array(inner) => {
                out.push_str(&format!("{lead}-\n"));
                emit_seq(inner, indent + 2, &spaces(indent + 2), out);
            }
            scalar => {
                out.push_str(&format!("{lead}- {}\n", scalar_text(scalar)));
            }
        }
    }
}

/// The YAML text for one scalar node: `!!js <expr>` for expressions, plain
/// when safe, single-quoted for strings needing quoting, double-quoted for
/// control characters.
fn scalar_text(node: &Node) -> String {
    match node {
        Node::Null => "null".to_owned(),
        Node::Bool(value) => value.to_string(),
        Node::Int(value) => value.to_string(),
        Node::UInt(value) => value.to_string(),
        Node::Float(value) => float_text(*value),
        Node::String(value) => quote_scalar(value),
        Node::Expr(value) => format!("!!js {}", quote_scalar(value)),
        Node::Array(_) | Node::Object(_) => unreachable!("scalars only"),
    }
}

/// Float rendering matching the previous writer: shortest round-trip
/// form, `.0` on integral values, YAML's `.inf`/`.nan` spellings.
fn float_text(value: f64) -> String {
    if value.is_nan() {
        ".nan".to_owned()
    } else if value == f64::INFINITY {
        ".inf".to_owned()
    } else if value == f64::NEG_INFINITY {
        "-.inf".to_owned()
    } else {
        let mut buffer = ryu::Buffer::new();
        buffer.format_finite(value).to_owned()
    }
}

/// How a string scalar must be quoted.
enum Quote {
    Plain,
    Single,
    Double,
}

fn classify(text: &str) -> Quote {
    if text.is_empty() {
        return Quote::Single;
    }
    if text.chars().any(|c| (c as u32) < 0x20 || c as u32 == 0x7f) {
        return Quote::Double;
    }
    let first = text.chars().next().expect("non-empty");
    if "#%,[]{}&*!|>'\"`@".contains(first) {
        return Quote::Single;
    }
    if (first == '-' || first == '?') && text.chars().nth(1).is_none_or(|next| next == ' ') {
        return Quote::Single;
    }
    if text == "---" || text == "..." {
        return Quote::Single;
    }
    if text.ends_with(':')
        || text.contains(": ")
        || text.contains(" #")
        || text.starts_with(' ')
        || text.ends_with(' ')
    {
        return Quote::Single;
    }
    // A string that would resolve as null, bool, number, or float must be
    // quoted to stay a string.
    if parse_null(text).is_some()
        || parse_bool(text).is_some()
        || parse_f64(text).is_some()
        || matches!(try_int(text), Ok(Some(_)))
    {
        return Quote::Single;
    }
    Quote::Plain
}

fn quote_scalar(text: &str) -> String {
    match classify(text) {
        Quote::Plain => text.to_owned(),
        Quote::Single => format!("'{}'", text.replace('\'', "''")),
        Quote::Double => double_quote(text),
    }
}

fn double_quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            other if (other as u32) < 0x20 || other as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", other as u32));
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse through the dialect and through the serde oracle; both must
    /// agree on the tree (compared by Debug text: NaN ≠ NaN under PartialEq).
    fn parity(source: &str) {
        let mine = parse_node(source).expect("dialect parse");
        let oracle: Node = serde_yaml_ng::from_str(source).expect("oracle parse");
        assert_eq!(
            format!("{mine:?}"),
            format!("{oracle:?}"),
            "dialect and serde disagree on {source:?}"
        );
    }

    #[test]
    fn scalar_resolution_matches_the_serde_oracle() {
        for value in [
            "null",
            "Null",
            "NULL",
            "~",
            "true",
            "True",
            "TRUE",
            "false",
            "False",
            "FALSE",
            "0",
            "-0",
            "42",
            "+5",
            "-17",
            "9223372036854775807",
            "9223372036854775808",
            "18446744073709551615",
            "0x1A",
            "0o17",
            "0b101",
            "-0x10",
            "007",
            "1_000",
            "1.5",
            "-0.0",
            "1e300",
            ".inf",
            "-.inf",
            ".nan",
            "yes",
            "No",
            "text",
            "http://x",
            "a b",
            "${{ env.X }}",
        ] {
            parity(&format!("key: {value}\n"));
            parity(&format!("- {value}\n"));
        }
        for value in ["", "0", "true", "42", " x "] {
            parity(&format!("key: '{value}'\n"));
            parity(&format!("key: \"{value}\"\n"));
        }
        parity("key:\n");
        parity("key: |\n  line1\n  line2\n");
        parity("key: >\n  folded text\n");
        parity("");
        parity("---\n");
    }

    #[test]
    fn structural_shapes_match_the_serde_oracle() {
        parity("[]\n");
        parity("{}\n");
        parity("- 1\n- a\n- [1, 2]\n- {x: 1}\n");
        parity("a:\n  b:\n    c: 1\n");
        parity("a: &anchor 1\nb: *anchor\n");
        parity("base: &b\n  x: 1\nover: *b\n");
        parity("list:\n- 1\n- 2\n");
        parity("- id: a\n  name: n\n  group:\n  - id: c\n");
        // Plain non-string keys coerce to their text, like the serde path.
        parity("m:\n  1: value\n");
    }

    #[test]
    fn core_tags_match_the_serde_oracle() {
        parity("a: !!str 5\n");
        parity("a: !!bool 'true'\n");
        parity("a: !!int 0x1f\n");
        parity("a: !!float 5\n");
        parity("a: !!null ~\n");
        parity("a: !!python/object 'x'\n");
    }

    /// Local `!tags` are serde enum syntax — the oracle rejects them for
    /// `Node`, and so does the dialect (with a clearer message).
    #[test]
    fn local_tags_fail_like_the_serde_path() {
        for source in ["a: !local 5\n", "a: !local 'x'\n"] {
            assert!(serde_yaml_ng::from_str::<Node>(source).is_err(), "{source}");
            let error = parse_node(source).unwrap_err().to_string();
            assert!(error.contains("local tags are not supported"), "{error}");
        }
    }

    /// Complex (non-scalar) mapping keys are errors on both paths.
    #[test]
    fn complex_mapping_keys_fail_like_the_serde_path() {
        let source = "? [a]\n: v\n";
        assert!(serde_yaml_ng::from_str::<Node>(source).is_err());
        let error = parse_node(source).unwrap_err().to_string();
        assert!(error.contains("mapping keys must be scalars"), "{error}");
    }

    #[test]
    fn js_tag_becomes_an_expression_node() {
        let node = parse_node("- id: a\n  config:\n    model: !!js process.env.MODEL\n").unwrap();
        let entry = &node.as_array().unwrap()[0];
        let config = entry.as_object().unwrap()["config"].as_object().unwrap();
        assert_eq!(config["model"], Node::Expr("process.env.MODEL".to_owned()));
        // Quoted !!js scalars are expressions too (js-yaml resolves any
        // string scalar for the type).
        let node = parse_node("k: !!js 'quoted expression'\n").unwrap();
        assert_eq!(
            node.as_object().unwrap()["k"],
            Node::Expr("quoted expression".to_owned())
        );
    }

    #[test]
    fn integer_overflow_fails_like_the_serde_path() {
        let error = parse_node("a: 18446744073709551616\n")
            .unwrap_err()
            .to_string();
        assert!(error.contains("integer out of range"), "{error}");
        // The same value through the oracle also fails.
        assert!(serde_yaml_ng::from_str::<Node>("a: 18446744073709551616\n").is_err());
    }

    #[test]
    fn syntax_errors_carry_line_and_column() {
        // libyaml anchors unclosed-flow errors at the position where the
        // problem became unavoidable (here, the start of the line after).
        let error = parse_node("a: [unclosed\n").unwrap_err().to_string();
        assert!(error.contains("line 2"), "{error}");
        let error = parse_node("valid: 1\nbroken: [x\n")
            .unwrap_err()
            .to_string();
        assert!(error.contains("line 3"), "{error}");
    }

    #[test]
    fn multiple_documents_and_unknown_anchors_fail() {
        let error = parse_node("---\na: 1\n---\nb: 2\n")
            .unwrap_err()
            .to_string();
        assert!(error.contains("more than one document"), "{error}");
        let error = parse_node("a: *missing\n").unwrap_err().to_string();
        assert!(error.contains("unknown anchor"), "{error}");
    }

    #[test]
    fn converters_reject_wrong_shapes_with_field_context() {
        let error = parse_document("- id: a\n").unwrap_err().to_string();
        assert!(
            error.contains("expected a mapping with an `entries` list"),
            "{error}"
        );
        let error = parse_document("entries: 5\n").unwrap_err().to_string();
        assert!(error.contains("entries:"), "{error}");
        let error = parse_entry_list("entries:\n  - id: a\n")
            .unwrap_err()
            .to_string();
        assert!(error.contains("expected a sequence of entries"), "{error}");
        let error = parse_entry_list("- name: 5\n").unwrap_err().to_string();
        assert!(
            error.contains("entry 1: name: expected a string"),
            "{error}"
        );
        let error = parse_entry_list("- inject: [1]\n").unwrap_err().to_string();
        assert!(error.contains("inject"), "{error}");
        let error = parse_entry_list("- disabled: maybe\n")
            .unwrap_err()
            .to_string();
        assert!(error.contains("disabled: expected a boolean"), "{error}");
        let error = parse_entry_list("- disabled: !!js process.platform\n")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("disabled: !!js expressions are not supported yet"),
            "{error}"
        );
    }

    #[test]
    fn converters_keep_unknown_entry_keys_dropped_and_patch_extras() {
        let entries = parse_entry_list("- id: a\n  name: n\n  mystery: value\n").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "n");

        let patches = crate::yaml::patch_list_from_node(
            parse_node("- id: a\n  intercept: db\n  group: []\n").unwrap(),
        )
        .unwrap();
        assert_eq!(patches[0].extra.len(), 2);
        assert!(patches[0].extra.contains_key("intercept"));
        assert!(patches[0].extra.contains_key("group"));

        let patches = crate::yaml::patch_list_from_node(
            parse_node("- insert: [{id: x, name: n}]\n").unwrap(),
        )
        .unwrap();
        assert_eq!(
            patches[0].insert.as_ref().unwrap()[0].id.as_deref(),
            Some("x")
        );
    }

    #[test]
    fn document_round_trips_with_extras_in_order() {
        let document = parse_document(
            "entries:\n  - id: a\n    name: n\n    config:\n      x: 1\nmeta: kept\n",
        )
        .unwrap();
        assert_eq!(document.entries.len(), 1);
        assert_eq!(document.extra.len(), 1);
        let text = emit_document(&document);
        assert_eq!(parse_document(&text).unwrap(), document, "{text}");
        assert!(text.contains("meta: kept"), "{text}");
    }

    #[test]
    fn emitter_matches_the_previous_writer_layout() {
        let entries = parse_entry_list(
            "- id: w1\n  name: worker\n  config: 8080\n- id: g\n  name: group\n  group:\n  - id: c1\n    name: adapter-http\n    config:\n      host: localhost\n      empty: ''\n      list:\n      - 1\n      - 2.5\n",
        )
        .unwrap();
        let text = emit_entry_list(&entries);
        let expected = "\
- id: w1
  name: worker
  config: 8080
- id: g
  name: group
  group:
  - id: c1
    name: adapter-http
    config:
      host: localhost
      empty: ''
      list:
      - 1
      - 2.5
";
        assert_eq!(text, expected);
        assert_eq!(parse_entry_list(&text).unwrap(), entries);
    }

    #[test]
    fn emitter_quotes_and_numbers_match_the_previous_writer() {
        for (text, expected) in [
            ("x: -foo\n", "x: -foo\n"),
            ("x: '#hash'\n", "x: '#hash'\n"),
            ("x: 'trailing:'\n", "x: 'trailing:'\n"),
            ("x: 'true'\n", "x: 'true'\n"),
            ("x: '0x1A'\n", "x: '0x1A'\n"),
            ("x: it's\n", "x: it's\n"),
            ("x: qu\"ote\n", "x: qu\"ote\n"),
            ("x: http://x\n", "x: http://x\n"),
            ("x: ${{ env.X }}\n", "x: ${{ env.X }}\n"),
            ("x: 0.5\n", "x: 0.5\n"),
            ("x: -0.0\n", "x: -0.0\n"),
            ("x: 1e300\n", "x: 1e300\n"),
            ("x: 8080\n", "x: 8080\n"),
        ] {
            let node = parse_node(text).unwrap();
            let emitted = emit_node_test(&node);
            assert_eq!(emitted, expected, "input {text:?}");
            assert_eq!(parse_node(&emitted).unwrap(), node, "round trip {text:?}");
        }
        // The empty document renders like the serde writer's.
        assert_eq!(emit_document(&Document::default()), "{}\n");
        // Empty containers stay inline; strings with control characters go
        // double-quoted and round-trip.
        let node = parse_node("a: []\nb: {}\nc: \"tab\\there\"\n").unwrap();
        let emitted = emit_node_test(&node);
        assert_eq!(emitted, "a: []\nb: {}\nc: \"tab\\there\"\n");
        assert_eq!(parse_node(&emitted).unwrap(), node);
    }

    fn emit_node_test(node: &Node) -> String {
        let mut out = String::new();
        emit_node(node, 0, "", &mut out);
        out
    }

    #[test]
    fn expressions_emit_and_round_trip() {
        let entries = parse_entry_list(
            "- id: a\n  name: n\n  config:\n    model: !!js process.env.MODEL || 'default'\n",
        )
        .unwrap();
        let text = emit_entry_list(&entries);
        assert!(
            text.contains("model: !!js process.env.MODEL || 'default'\n"),
            "{text}"
        );
        assert_eq!(parse_entry_list(&text).unwrap(), entries);
    }

    /// A bundle-shaped fixture: every field shape the shipped patch files
    /// use (`id`, `name`, `inject`, `config`, `!!js`), node-level
    /// round-tripped (the `disabled: !!js` form needs the typed slot the
    /// next stage adds, so it stays at node level for now).
    #[test]
    fn bundle_shaped_fixture_round_trips() {
        let fixture = "\
- id: base-sandbox
  name: '@dsh/base'
  inject: [database]
  config:
    level: !!js process.env.DSH_LOG_LEVEL || 'info'
    retries: 3
    nested:
      keep: true
- insert:
    - id: web-app
      name: '@dsh/web-app'
      config:
        theme: dark
        gate: !!js process.platform === 'darwin'
";
        let node = parse_node(fixture).unwrap();
        let emitted = {
            let mut out = String::new();
            emit_node(&node, 0, "", &mut out);
            out
        };
        assert_eq!(parse_node(&emitted).unwrap(), node, "{emitted}");
        assert!(
            emitted.contains("!!js process.platform === 'darwin'"),
            "{emitted}"
        );

        let patches = crate::yaml::patch_list_from_node(parse_node(fixture).unwrap()).unwrap();
        assert_eq!(patches.len(), 2);
        assert_eq!(
            patches[1].insert.as_ref().unwrap()[0].id.as_deref(),
            Some("web-app")
        );
    }

    #[test]
    fn anchors_resolve_into_shared_clones() {
        let node = parse_node("a: &x {v: 1}\nb: *x\n").unwrap();
        let map = node.as_object().unwrap();
        assert_eq!(map["a"], map["b"]);
    }
}
