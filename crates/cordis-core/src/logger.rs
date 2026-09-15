//! Runtime-local diagnostic logging through exact exporter occurrences.
//!
//! [`Logger`] is a core foundation facility, not a Service. Each call creates one
//! immutable [`LogRecord`], assigns its Runtime-local sequence and [`SystemTime`],
//! snapshots the current exporter occurrences, and then filters/invokes them with
//! no exporter-store lock held. Exporter registrations are exact, generation-owned
//! occurrences controlled by move-only [`ExporterRegistration`] capabilities.

use crate::context::Context;
use parking_lot::Mutex;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

/// Semantic log severity, ordered from least to most severe.
///
/// This is intentionally not a numeric enum: callers can compare semantic
/// severities but cannot reinterpret one as a wire/storage integer.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Level(LevelKind);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum LevelKind {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    /// Verbose diagnostic detail.
    #[allow(non_upper_case_globals)]
    pub const Debug: Self = Self(LevelKind::Debug);
    /// Routine operational message.
    #[allow(non_upper_case_globals)]
    pub const Info: Self = Self(LevelKind::Info);
    /// Recoverable anomaly worth surfacing.
    #[allow(non_upper_case_globals)]
    pub const Warn: Self = Self(LevelKind::Warn);
    /// Operation-blocking failure.
    #[allow(non_upper_case_globals)]
    pub const Error: Self = Self(LevelKind::Error);

    /// Stable lower-case severity name.
    pub fn as_str(self) -> &'static str {
        match self.0 {
            LevelKind::Debug => "debug",
            LevelKind::Info => "info",
            LevelKind::Warn => "warn",
            LevelKind::Error => "error",
        }
    }
}

impl fmt::Debug for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0 {
            LevelKind::Debug => "Debug",
            LevelKind::Info => "Info",
            LevelKind::Warn => "Warn",
            LevelKind::Error => "Error",
        })
    }
}

/// One immutable Runtime-local log record.
#[derive(Debug, Clone)]
pub struct LogRecord {
    sequence: u64,
    timestamp: SystemTime,
    channel: String,
    level: Level,
    text: String,
}

impl LogRecord {
    /// Runtime-local record-assignment sequence.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    /// Wall-clock time captured when this record was assigned.
    pub fn timestamp(&self) -> SystemTime {
        self.timestamp
    }
    /// Logger channel that emitted this record.
    pub fn channel(&self) -> &str {
        &self.channel
    }
    /// Semantic record severity.
    pub fn level(&self) -> Level {
        self.level
    }
    /// Rendered record text.
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// A sink for immutable log records.
///
/// `min_level(channel) == None` means use [`Exporter::default_level`].
/// Logger invokes filtering and export only after releasing its occurrence-store
/// lock, and contains panics per occurrence so one broken exporter cannot block
/// later attempts.
pub trait Exporter: Send + Sync {
    /// Receive one record that passed this exporter's threshold.
    fn export(&self, record: &LogRecord);
    /// Optional channel-specific threshold; `None` falls back to the default.
    fn min_level(&self, _channel: &str) -> Option<Level> {
        None
    }
    /// Fallback threshold for channels without an override.
    fn default_level(&self) -> Level {
        Level::Info
    }
}

/// A named Runtime logging channel sharing one Runtime-wide exporter set.
#[derive(Clone)]
pub struct Logger {
    name: String,
    service: Arc<LoggerService>,
}

impl fmt::Debug for Logger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Logger").field("name", &self.name).finish()
    }
}

thread_local! {
    static EXPORTER_CALLBACK_STACK: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

impl Logger {
    /// Assign one immutable record and synchronously attempt the current exporter snapshot.
    pub fn log(&self, level: Level, text: impl fmt::Display) {
        let sequence = self.service.next_sequence();
        let record = LogRecord {
            sequence,
            timestamp: SystemTime::now(),
            channel: self.name.clone(),
            level,
            text: text.to_string(),
        };
        self.service.dispatch(record);
    }
    /// Log at [`Level::Error`].
    pub fn error(&self, text: impl fmt::Display) {
        self.log(Level::Error, text);
    }
    /// Log at [`Level::Warn`].
    pub fn warn(&self, text: impl fmt::Display) {
        self.log(Level::Warn, text);
    }
    /// Log at [`Level::Info`].
    pub fn info(&self, text: impl fmt::Display) {
        self.log(Level::Info, text);
    }
    /// Log at [`Level::Debug`].
    pub fn debug(&self, text: impl fmt::Display) {
        self.log(Level::Debug, text);
    }
    /// Derive another channel over the same Runtime exporter set.
    #[must_use = "a derived logger that is dropped logs nothing"]
    pub fn with_name(&self, name: impl Into<String>) -> Logger {
        Logger {
            name: name.into(),
            service: self.service.clone(),
        }
    }
    /// This logger's channel name.
    pub fn name(&self) -> &str {
        &self.name
    }
    #[cfg(test)]
    pub(crate) fn new_for_test(service: Arc<LoggerService>) -> Self {
        Self {
            name: "contained-test".to_owned(),
            service,
        }
    }
}

/// Rejection for zero-capacity [`BufferExporter`] construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("buffer_size must be greater than zero")]
pub struct BufferSizeZero;

/// Explicitly installed bounded in-memory exporter.
///
/// Records are retained in exporter receipt order, oldest to newest. This type
/// intentionally has no `Clone`, `Default`, serde, or automatic installation.
pub struct BufferExporter {
    capacity: usize,
    buffer: Mutex<VecDeque<LogRecord>>,
    min_level: Level,
}

impl BufferExporter {
    /// Construct an opt-in bounded buffer with a nonzero capacity.
    pub fn new(capacity: usize, min_level: Level) -> std::result::Result<Self, BufferSizeZero> {
        if capacity == 0 {
            return Err(BufferSizeZero);
        }
        Ok(Self {
            capacity,
            buffer: Mutex::new(VecDeque::new()),
            min_level,
        })
    }
    /// Snapshot retained records in receipt order, oldest first.
    pub fn snapshot(&self) -> Vec<LogRecord> {
        self.buffer.lock().iter().cloned().collect()
    }
    /// Remove all retained records.
    pub fn clear(&self) {
        self.buffer.lock().clear();
    }
}

impl Exporter for BufferExporter {
    fn export(&self, record: &LogRecord) {
        let mut buffer = self.buffer.lock();
        if buffer.len() == self.capacity {
            buffer.pop_front();
        }
        buffer.push_back(record.clone());
    }
    fn default_level(&self) -> Level {
        self.min_level
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExporterKey(u64);

struct ExporterOccurrence {
    key: ExporterKey,
    exporter: Arc<dyn Exporter>,
}

/// Move-only control for one exact exporter occurrence.
///
/// Dropping this value is inert: generation cleanup remains the owner of an
/// unremoved occurrence. [`remove`](Self::remove) consumes the control.
pub struct ExporterRegistration {
    service: Arc<LoggerService>,
    key: ExporterKey,
}

impl fmt::Debug for ExporterRegistration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExporterRegistration")
            .finish_non_exhaustive()
    }
}

impl ExporterRegistration {
    /// Remove this exact occurrence if it is still current.
    #[must_use]
    pub fn remove(self) -> bool {
        self.service.remove_exporter(self.key)
    }
}

pub(crate) struct LoggerService {
    sequence: AtomicU64,
    exporter_sequence: AtomicU64,
    exporters: Mutex<Vec<ExporterOccurrence>>,
}

impl Default for LoggerService {
    fn default() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            exporter_sequence: AtomicU64::new(0),
            exporters: Mutex::new(Vec::new()),
        }
    }
}

impl LoggerService {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn logger_for_fiber(self: &Arc<Self>, fiber_name: &str) -> Logger {
        Logger {
            name: hyphenate(fiber_name),
            service: self.clone(),
        }
    }

    fn next_sequence(&self) -> u64 {
        self.sequence
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .expect("log record sequence space exhausted")
            + 1
    }

    pub(crate) fn reserve_exporter(&self) -> ExporterKey {
        let previous = self
            .exporter_sequence
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .expect("exporter occurrence identity space exhausted");
        ExporterKey(previous + 1)
    }

    pub(crate) fn insert_reserved(&self, key: ExporterKey, exporter: Arc<dyn Exporter>) {
        self.exporters
            .lock()
            .push(ExporterOccurrence { key, exporter });
    }

    fn remove_exporter(&self, key: ExporterKey) -> bool {
        let removed = {
            let mut list = self.exporters.lock();
            list.iter()
                .position(|entry| entry.key == key)
                .map(|index| list.remove(index))
        };
        removed.is_some()
    }

    fn dispatch(&self, record: LogRecord) {
        let identity = self as *const Self as usize;
        if EXPORTER_CALLBACK_STACK.with(|stack| stack.borrow().contains(&identity)) {
            eprintln!(
                "cordis: reentrant log suppressed [{}] {}: {}",
                record.level.as_str(),
                record.channel,
                record.text
            );
            return;
        }
        let exporters: Vec<Arc<dyn Exporter>> = self
            .exporters
            .lock()
            .iter()
            .map(|entry| entry.exporter.clone())
            .collect();
        for exporter in exporters {
            crate::contained::contain("log exporter", None, || {
                EXPORTER_CALLBACK_STACK.with(|stack| {
                    struct Reset<'a>(&'a RefCell<Vec<usize>>, usize);
                    impl Drop for Reset<'_> {
                        fn drop(&mut self) {
                            let popped = self.0.borrow_mut().pop();
                            debug_assert_eq!(popped, Some(self.1));
                        }
                    }
                    stack.borrow_mut().push(identity);
                    let _reset = Reset(stack, identity);
                    let threshold = exporter
                        .min_level(record.channel())
                        .unwrap_or_else(|| exporter.default_level());
                    if record.level() >= threshold {
                        exporter.export(&record);
                    }
                });
            });
        }
    }
}

impl Context {
    /// Obtain the current Fiber's default hyphenated Logger channel.
    pub fn logger(&self) -> Logger {
        self.root.logger.logger_for_fiber(&self.fiber.name)
    }

    /// Atomically publish one exact exporter occurrence and generation cleanup.
    pub fn add_exporter(
        &self,
        exporter: Arc<dyn Exporter>,
    ) -> std::result::Result<ExporterRegistration, crate::effect::EffectRegistrationError> {
        let service = self.root.logger.clone();
        let key = service.reserve_exporter();
        let mut step = ExporterPublish {
            service: &self.root.logger,
            key,
            exporter: Some(exporter),
        };
        let cleanup_service = service.clone();
        crate::gated::push_gated(
            self.fiber(),
            crate::effect::fut_cleanup(move || {
                Box::pin(async move {
                    cleanup_service.remove_exporter(key);
                })
            }),
            &mut step,
        )
        .map_err(|_| crate::effect::EffectRegistrationError::InactiveContext)?;
        Ok(ExporterRegistration { service, key })
    }
}

pub(crate) struct ExporterPublish<'a> {
    service: &'a LoggerService,
    key: ExporterKey,
    exporter: Option<Arc<dyn Exporter>>,
}

impl crate::gated::PublishStep for ExporterPublish<'_> {
    fn publish(&mut self) -> std::result::Result<(), crate::gated::PublishRefused> {
        self.service
            .insert_reserved(self.key, self.exporter.take().expect("publish runs once"));
        Ok(())
    }
}

/// Convert `PascalCase` / `camelCase` / `snake_case` to `kebab-case` —
/// the logger-name fallback, a faithful port of cosmokit-1.8.1
/// `tokenize(source, [45, 95], 45)` (the tokenizer behind its
/// `paramCase`, aliased `hyphenate`): `-` and `_` are delimiters
/// collapsed into a single `-`, leading delimiters are stripped, digits
/// and other characters pass through *without* resetting the LOWER state
/// (so `foo2B` still splits at the `B`), and an UPPER→lower boundary
/// splits only when the next char is lowercase (`HTTPServer` →
/// `http-server`).
fn hyphenate(name: &str) -> String {
    /// The tokenizer state (cosmokit's `State` enum).
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Delim,
        Upper,
        Lower,
    }
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::with_capacity(name.len() + 4);
    let mut state = State::Delim;
    for (i, &ch) in chars.iter().enumerate() {
        if ch.is_ascii_uppercase() {
            if state == State::Upper {
                // UPPER run: split before the capital that starts a new
                // lowercase word ("HTTPServer" splits before "Server")
                if chars.get(i + 1).is_some_and(|c| c.is_ascii_lowercase()) {
                    out.push('-');
                }
            } else if state != State::Delim {
                out.push('-');
            }
            out.push(ch.to_ascii_lowercase());
            state = State::Upper;
        } else if ch.is_ascii_lowercase() {
            out.push(ch);
            state = State::Lower;
        } else if ch == '-' || ch == '_' {
            // delimiters collapse into one '-'; leading ones are stripped
            if state != State::Delim {
                out.push('-');
            }
            state = State::Delim;
        } else {
            // digits and anything else pass through WITHOUT resetting the
            // state, so "foo2B" / "x9Y" still split at the capital
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::hyphenate;
    #[test]
    fn hyphenates_names_like_cosmokit() {
        assert_eq!(hyphenate("FooBar"), "foo-bar");
        assert_eq!(hyphenate("fooBar"), "foo-bar");
        assert_eq!(hyphenate("HTTPServer"), "http-server");
        assert_eq!(hyphenate("root"), "root");
        assert_eq!(hyphenate("foo_bar"), "foo-bar");
        assert_eq!(hyphenate("Foo--Bar"), "foo-bar");
        assert_eq!(hyphenate("-Foo"), "foo");
        assert_eq!(hyphenate("foo2B"), "foo2-b");
        assert_eq!(hyphenate("x9Y"), "x9-y");
        assert_eq!(hyphenate("foo_"), "foo-");
        assert_eq!(hyphenate("Bar-"), "bar-");
        assert_eq!(hyphenate("audit/Probe"), "audit/-probe");
    }
}
