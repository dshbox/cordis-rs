//! Structured logger facade, bounded buffer, formatting, and exporters.

use crate::context::Context;
use crate::effect::{AsyncDisposer, EffectHandle};
use crate::utils::lock;
use crate::{Result, Value};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt::{self, Debug, Formatter};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Logger severity/method name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LoggerType {
    /// Error message.
    Error,
    /// Informational message.
    Info,
    /// Warning message.
    Warn,
    /// Debug message.
    Debug,
}

/// Numeric exporter threshold, matching Cordis ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum LoggerLevel {
    /// Error only.
    Error = 0,
    /// Errors and info.
    Info = 1,
    /// Errors, info, and warnings.
    Warn = 2,
    /// Every built-in level.
    Debug = 3,
}

impl LoggerType {
    fn level(self) -> LoggerLevel {
        match self {
            Self::Error => LoggerLevel::Error,
            Self::Info => LoggerLevel::Info,
            Self::Warn => LoggerLevel::Warn,
            Self::Debug => LoggerLevel::Debug,
        }
    }
}

/// Dynamically typed but formatting-friendly log argument.
#[derive(Debug, Clone)]
pub enum LogArg {
    /// UTF-8 string.
    String(String),
    /// Signed integer.
    Integer(i64),
    /// Unsigned integer.
    Unsigned(u64),
    /// Floating point number.
    Float(f64),
    /// Boolean.
    Bool(bool),
    /// Pre-rendered debug/object representation.
    Object(String),
}

impl LogArg {
    fn string(&self) -> String {
        match self {
            Self::String(value) | Self::Object(value) => value.clone(),
            Self::Integer(value) => value.to_string(),
            Self::Unsigned(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
            Self::Bool(value) => value.to_string(),
        }
    }

    fn integer(&self) -> String {
        match self {
            Self::Integer(value) => value.to_string(),
            Self::Unsigned(value) => value.to_string(),
            Self::Float(value) => (*value as i64).to_string(),
            Self::Bool(value) => (if *value { 1_i64 } else { 0_i64 }).to_string(),
            Self::String(value) | Self::Object(value) => {
                value.parse::<i64>().unwrap_or_default().to_string()
            }
        }
    }

    fn float(&self) -> String {
        match self {
            Self::Float(value) => value.to_string(),
            Self::Integer(value) => (*value as f64).to_string(),
            Self::Unsigned(value) => (*value as f64).to_string(),
            Self::Bool(value) => (if *value { 1.0 } else { 0.0 }).to_string(),
            Self::String(value) | Self::Object(value) => {
                value.parse::<f64>().unwrap_or_default().to_string()
            }
        }
    }
}

impl From<String> for LogArg {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for LogArg {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

macro_rules! integer_log_arg {
    ($($ty:ty),* $(,)?) => {$ (
        impl From<$ty> for LogArg {
            fn from(value: $ty) -> Self { Self::Integer(value as i64) }
        }
    )* };
}
integer_log_arg!(i8, i16, i32, i64, isize);

macro_rules! unsigned_log_arg {
    ($($ty:ty),* $(,)?) => {$ (
        impl From<$ty> for LogArg {
            fn from(value: $ty) -> Self { Self::Unsigned(value as u64) }
        }
    )* };
}
unsigned_log_arg!(u8, u16, u32, u64, usize);

impl From<f32> for LogArg {
    fn from(value: f32) -> Self {
        Self::Float(value.into())
    }
}

impl From<f64> for LogArg {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

impl From<bool> for LogArg {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<Value> for LogArg {
    fn from(value: Value) -> Self {
        Self::Object(format!("{value:?}"))
    }
}

/// Structured log record delivered to exporters.
#[derive(Debug, Clone)]
pub struct Message {
    /// Monotonic sequence number.
    pub sequence: u64,
    /// Unix timestamp in milliseconds.
    pub timestamp: u64,
    /// Logger name.
    pub name: String,
    /// Severity category.
    pub kind: LoggerType,
    /// Numeric severity.
    pub level: LoggerLevel,
    /// Printf-style format plus values.
    pub args: Vec<LogArg>,
    /// Originating fiber id.
    pub fiber_uid: Option<u64>,
    /// Originating fiber display name.
    pub fiber_name: String,
}

/// Custom placeholder formatter.
pub type FormatterFn =
    Arc<dyn Fn(&LogArg, &ExporterConfig, &Message) -> String + Send + Sync + 'static>;

/// Exporter formatting and level configuration.
#[derive(Clone)]
pub struct ExporterConfig {
    /// ANSI color capability (`0` disables, `2+` enables decorations).
    pub colors: u8,
    /// Maximum Unicode scalar count for each rendered line.
    pub max_length: usize,
    /// Logger-specific thresholds. The `default` key is the fallback.
    pub levels: HashMap<String, LoggerLevel>,
    /// Additional or overriding printf placeholder formatters.
    pub formatters: HashMap<char, FormatterFn>,
}

impl Default for ExporterConfig {
    fn default() -> Self {
        Self {
            colors: 0,
            max_length: 10_240,
            levels: HashMap::new(),
            formatters: HashMap::new(),
        }
    }
}

impl Debug for ExporterConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExporterConfig")
            .field("colors", &self.colors)
            .field("max_length", &self.max_length)
            .field("levels", &self.levels)
            .field("formatters", &self.formatters.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Structured log sink.
pub trait Exporter: Send + Sync + 'static {
    /// Exporter-specific formatting and filtering.
    fn config(&self) -> ExporterConfig {
        ExporterConfig::default()
    }

    /// Borrow the config without cloning, when the exporter stores it.
    ///
    /// The logger hot path prefers this over [`config`](Self::config), which
    /// deep-clones the level and formatter maps for closure exporters.
    fn config_ref(&self) -> Option<&ExporterConfig> {
        None
    }

    /// Receive one message.
    fn export(&self, message: &Message);
}

struct ClosureExporter<F> {
    config: ExporterConfig,
    callback: F,
}

impl<F> Exporter for ClosureExporter<F>
where
    F: Fn(&Message) + Send + Sync + 'static,
{
    fn config(&self) -> ExporterConfig {
        self.config.clone()
    }

    fn config_ref(&self) -> Option<&ExporterConfig> {
        Some(&self.config)
    }

    fn export(&self, message: &Message) {
        (self.callback)(message)
    }
}

/// Logger intercept config resolved from `ctx.intercept("logger", ...)`.
#[derive(Debug, Clone, Default)]
pub struct LoggerIntercept {
    /// Override derived logger name.
    pub name: Option<String>,
    /// Override default level.
    pub level: Option<LoggerLevel>,
}

struct LoggerState {
    sequence: u64,
    next_exporter: u64,
    buffer_size: usize,
    buffer: VecDeque<Arc<Message>>,
    exporters: BTreeMap<u64, Arc<dyn Exporter>>,
    exporter_snapshot: Arc<Vec<Arc<dyn Exporter>>>,
}

fn refresh_exporter_snapshot(state: &mut LoggerState) {
    state.exporter_snapshot = Arc::new(state.exporters.values().cloned().collect());
}

pub(crate) struct LoggerRoot {
    state: Mutex<LoggerState>,
}

impl LoggerRoot {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(LoggerState {
                sequence: 0,
                next_exporter: 0,
                buffer_size: 1_000,
                buffer: VecDeque::new(),
                exporters: BTreeMap::new(),
                exporter_snapshot: Arc::new(Vec::new()),
            }),
        }
    }

    /// Whether anything would consume a record of `kind`: the bounded
    /// buffer (gated by `default_level`) or at least one exporter. Cheap on
    /// purpose — a short lock probe, no allocation — so `Logger::write` can
    /// bail out before assembling arguments. Per-exporter thresholds may
    /// still drop the record later; this only rules out records nobody
    /// could ever see.
    fn enabled(&self, default_level: Option<LoggerLevel>, kind: LoggerType) -> bool {
        let state = lock(&self.state);
        let buffered =
            state.buffer_size > 0 && default_level.unwrap_or(LoggerLevel::Info) >= kind.level();
        buffered || !state.exporter_snapshot.is_empty()
    }

    fn send(
        &self,
        name: String,
        default_level: Option<LoggerLevel>,
        kind: LoggerType,
        args: Vec<LogArg>,
        fiber_uid: Option<u64>,
        fiber_name: String,
    ) {
        let (message, exporters) = {
            let mut state = lock(&self.state);
            state.sequence += 1;
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .min(u64::MAX as u128) as u64;
            let message = Message {
                sequence: state.sequence,
                timestamp,
                name,
                kind,
                level: kind.level(),
                args,
                fiber_uid,
                fiber_name,
            };
            let buffer_threshold = default_level.unwrap_or(LoggerLevel::Info);
            let message = Arc::new(message);
            if buffer_threshold >= message.level && state.buffer_size > 0 {
                state.buffer.push_back(message.clone());
                while state.buffer.len() > state.buffer_size {
                    state.buffer.pop_front();
                }
            }
            (message, state.exporter_snapshot.clone())
        };

        for exporter in exporters.iter() {
            let owned_config;
            let config = match exporter.config_ref() {
                Some(config) => config,
                None => {
                    owned_config = exporter.config();
                    &owned_config
                }
            };
            let threshold = config
                .levels
                .get(&message.name)
                .or_else(|| config.levels.get("default"))
                .copied()
                .or(default_level)
                .unwrap_or(LoggerLevel::Info);
            if threshold >= message.level {
                exporter.export(&message);
            }
        }
    }
}

/// ANSI 16-color palette indexes used for logger name coloring.
pub const C16: &[u8] = &[6, 2, 3, 4, 5, 1];
/// ANSI 256-color palette indexes used for logger name coloring.
pub const C256: &[u8] = &[
    20, 21, 26, 27, 32, 33, 38, 39, 40, 41, 42, 43, 44, 45, 56, 57, 62, 63, 68, 69, 74, 75, 76, 77,
    78, 79, 80, 81, 92, 93, 98, 99, 112, 113, 129, 134, 135, 148, 149, 160, 161, 162, 163, 164,
    165, 166, 167, 168, 169, 170, 171, 172, 173, 178, 179, 184, 185, 196, 197, 198, 199, 200, 201,
    202, 203, 204, 205, 206, 207, 208, 209, 214, 215, 220, 221,
];

/// Stable logger-name color hash.
pub fn color_code(name: &str, colors: u8) -> u8 {
    let mut hash: i32 = 0;
    for byte in name.bytes() {
        hash = hash
            .wrapping_shl(3)
            .wrapping_sub(hash)
            .wrapping_add(i32::from(byte))
            .wrapping_add(13);
    }
    let palette = if colors == 0 {
        return 0;
    } else if colors >= 2 {
        C256
    } else {
        C16
    };
    palette[(hash.unsigned_abs() as usize) % palette.len()]
}

fn color(config: &ExporterConfig, code: u8, value: String) -> String {
    if config.colors == 0 {
        value
    } else if code < 8 {
        format!("\u{001b}[3{code}m{value}\u{001b}[0m")
    } else {
        format!("\u{001b}[38;5;{code}m{value}\u{001b}[0m")
    }
}

/// Format a message using Cordis printf placeholders.
pub fn default_format(config: &ExporterConfig, message: &Message) -> String {
    let mut values = message.args.iter();
    let format = match values.next() {
        Some(LogArg::String(value)) => value.clone(),
        Some(value) => {
            let mut output = value.string();
            for value in values {
                output.push(' ');
                output.push_str(&value.string());
            }
            return truncate_lines(output, config.max_length);
        }
        None => String::new(),
    };

    let remaining = values.cloned().collect::<Vec<_>>();
    let mut next_value = 0;
    let mut chars = format.chars().peekable();
    let mut output = String::new();
    while let Some(character) = chars.next() {
        if character != '%' {
            output.push(character);
            continue;
        }
        let Some(placeholder) = chars.next() else {
            output.push('%');
            break;
        };
        if placeholder == '%' {
            output.push('%');
            continue;
        }
        let Some(value) = remaining.get(next_value) else {
            output.push('%');
            output.push(placeholder);
            continue;
        };
        if let Some(formatter) = config.formatters.get(&placeholder) {
            output.push_str(&formatter(value, config, message));
            next_value += 1;
            continue;
        }
        let rendered = match placeholder {
            's' | 'o' | 'O' => Some(value.string()),
            'd' | 'i' => Some(value.integer()),
            'f' => Some(value.float()),
            'c' => Some(String::new()),
            'C' => Some(color(
                config,
                color_code(&message.name, config.colors),
                value.string(),
            )),
            _ => None,
        };
        if let Some(rendered) = rendered {
            output.push_str(&rendered);
            next_value += 1;
        } else {
            output.push('%');
            output.push(placeholder);
        }
    }
    for value in &remaining[next_value..] {
        output.push(' ');
        output.push_str(&value.string());
    }
    truncate_lines(output, config.max_length)
}

fn truncate_lines(value: String, max_length: usize) -> String {
    value
        .lines()
        .map(|line| {
            let mut chars = line.chars();
            let head = chars.by_ref().take(max_length).collect::<String>();
            if chars.next().is_some() {
                head + "..."
            } else {
                head
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Named logger facade.
#[derive(Clone)]
pub struct Logger {
    ctx: Context,
    /// Logger name.
    pub name: String,
    /// Default threshold if an exporter supplies none.
    pub level: Option<LoggerLevel>,
}

impl Logger {
    fn write(&self, kind: LoggerType, format: impl Into<String>, args: Vec<LogArg>) {
        // Fast path: when neither the bounded buffer nor any exporter would
        // consume this record, skip the argument assembly and fiber-name
        // resolution entirely — the level check inside send() would only
        // throw the message away afterwards.
        if !self.ctx.root.logger.enabled(self.level, kind) {
            return;
        }
        let mut all = Vec::with_capacity(args.len() + 1);
        all.push(LogArg::String(format.into()));
        all.extend(args);
        let fiber = self.ctx.fiber().ok();
        self.ctx.root.logger.send(
            self.name.clone(),
            self.level,
            kind,
            all,
            fiber.as_ref().and_then(|fiber| fiber.uid()),
            fiber
                .map(|fiber| fiber.name())
                .unwrap_or_else(|| "disposed".to_owned()),
        );
    }

    /// Log an error.
    pub fn error(&self, format: impl Into<String>, args: impl IntoIterator<Item = LogArg>) {
        self.write(LoggerType::Error, format, args.into_iter().collect());
    }

    /// Log an informational message.
    pub fn info(&self, format: impl Into<String>, args: impl IntoIterator<Item = LogArg>) {
        self.write(LoggerType::Info, format, args.into_iter().collect());
    }

    /// Log a warning.
    pub fn warn(&self, format: impl Into<String>, args: impl IntoIterator<Item = LogArg>) {
        self.write(LoggerType::Warn, format, args.into_iter().collect());
    }

    /// Log a debug message.
    pub fn debug(&self, format: impl Into<String>, args: impl IntoIterator<Item = LogArg>) {
        self.write(LoggerType::Debug, format, args.into_iter().collect());
    }
}

impl Debug for Logger {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Logger")
            .field("name", &self.name)
            .field("level", &self.level)
            .finish()
    }
}

/// Built-in logging service bound to a context.
#[derive(Clone, Debug)]
pub struct LoggerService {
    ctx: Context,
}

impl LoggerService {
    pub(crate) fn new(ctx: Context) -> Self {
        Self { ctx }
    }

    fn intercept(&self) -> LoggerIntercept {
        let mut resolved = LoggerIntercept::default();
        if let Ok(configs) = self.ctx.intercepts::<LoggerIntercept>("logger") {
            for config in configs {
                if config.name.is_some() {
                    resolved.name = config.name.clone();
                }
                if config.level.is_some() {
                    resolved.level = config.level;
                }
            }
        }
        resolved
    }

    /// Create a logger. An intercept name overrides the fiber-derived name;
    /// an explicit name overrides both.
    pub fn logger(&self, name: Option<String>) -> Logger {
        let intercept = self.intercept();
        let name = name
            .or(intercept.name)
            .or_else(|| self.ctx.fiber().ok().map(|fiber| hyphenate(&fiber.name())))
            .unwrap_or_else(|| "root".to_owned());
        Logger {
            ctx: self.ctx.clone(),
            name,
            level: intercept.level,
        }
    }

    /// Register an exporter owned by the current fiber.
    pub fn exporter<E: Exporter>(&self, exporter: E) -> Result<EffectHandle> {
        self.exporter_arc(Arc::new(exporter))
    }

    /// Register an exporter callback with explicit config.
    pub fn exporter_fn<F>(&self, config: ExporterConfig, callback: F) -> Result<EffectHandle>
    where
        F: Fn(&Message) + Send + Sync + 'static,
    {
        self.exporter(ClosureExporter { config, callback })
    }

    /// Register an already shared exporter.
    pub fn exporter_arc(&self, exporter: Arc<dyn Exporter>) -> Result<EffectHandle> {
        let id = {
            let mut state = lock(&self.ctx.root.logger.state);
            state.next_exporter += 1;
            let id = state.next_exporter;
            state.exporters.insert(id, exporter);
            refresh_exporter_snapshot(&mut state);
            id
        };
        let root = Arc::downgrade(&self.ctx.root);
        let effect = self.ctx.fiber()?.register_effect(
            "ctx.logger.exporter()",
            AsyncDisposer::from_sync(move || {
                if let Some(root) = root.upgrade() {
                    let mut state = lock(&root.logger.state);
                    state.exporters.remove(&id);
                    refresh_exporter_snapshot(&mut state);
                }
                Ok(())
            }),
        );
        if effect.is_err() {
            let mut state = lock(&self.ctx.root.logger.state);
            state.exporters.remove(&id);
            refresh_exporter_snapshot(&mut state);
        }
        effect
    }

    /// Snapshot the chronological bounded message buffer.
    pub fn buffer(&self) -> Vec<Message> {
        lock(&self.ctx.root.logger.state)
            .buffer
            .iter()
            .map(|message| message.as_ref().clone())
            .collect()
    }

    /// Set buffer capacity, immediately trimming oldest records.
    pub fn set_buffer_size(&self, size: usize) {
        let mut state = lock(&self.ctx.root.logger.state);
        state.buffer_size = size;
        while state.buffer.len() > size {
            state.buffer.pop_front();
        }
    }

    /// Current buffer capacity.
    pub fn buffer_size(&self) -> usize {
        lock(&self.ctx.root.logger.state).buffer_size
    }

    /// Number of registered exporters.
    pub fn exporter_count(&self) -> usize {
        lock(&self.ctx.root.logger.state).exporters.len()
    }

    /// Remove buffered messages without touching exporters.
    pub fn clear_buffer(&self) {
        lock(&self.ctx.root.logger.state).buffer.clear();
    }
}

/// CamelCase → kebab-case with acronym support: a `-` is inserted before an
/// uppercase run only when it follows a lowercase character or ends before a
/// lowercase one, so `"HTTPServer"` becomes `"http-server"` and `"MyClass"`
/// stays `"my-class"`.
fn hyphenate(value: &str) -> String {
    let mut output = String::new();
    let characters = value.chars().collect::<Vec<_>>();
    for (index, character) in characters.iter().copied().enumerate() {
        if !character.is_uppercase() {
            output.push(character);
            continue;
        }
        let previous = index.checked_sub(1).map(|index| characters[index]);
        let next = characters.get(index + 1).copied();
        if previous.is_some_and(|previous| {
            previous.is_lowercase()
                || (previous.is_uppercase() && next.is_some_and(char::is_lowercase))
        }) {
            output.push('-');
        }
        for lower in character.to_lowercase() {
            output.push(lower);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Context;

    #[test]
    fn hyphenate_splits_camel_case_and_acronyms() {
        assert_eq!(hyphenate("greeter"), "greeter");
        assert_eq!(hyphenate("MyClass"), "my-class");
        assert_eq!(hyphenate("HTTPServer"), "http-server");
        assert_eq!(hyphenate("ABC"), "abc");
        assert_eq!(hyphenate("A"), "a");
    }

    /// The filtered fast path must exactly predict what send() keeps: with
    /// no exporters, only records the buffer accepts are observable. Debug
    /// under the default info threshold is dropped before any allocation;
    /// records the buffer accepts still land in it.
    #[test]
    fn filtered_records_skip_the_buffer_entirely() {
        let root = Context::new();
        let service = root.logger_service();
        let logger = root.logger();
        assert_eq!(service.exporter_count(), 0);
        assert_eq!(service.buffer().len(), 0);

        logger.debug("dropped %d", [LogArg::Integer(1)]);
        assert_eq!(service.buffer().len(), 0, "debug below the default level");

        logger.error("kept %d", [LogArg::Integer(2)]);
        let buffer = service.buffer();
        assert_eq!(buffer.len(), 1);
        assert_eq!(buffer[0].kind, LoggerType::Error);
        assert!(matches!(
            &buffer[0].args[0],
            LogArg::String(text) if text == "kept %d"
        ));
    }
}
