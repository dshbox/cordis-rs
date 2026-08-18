//! Config files: format detection, ordered round-trips, atomic writes.

use crate::error::{IncludeError, Result};
use crate::lock;
use crate::node::Node;
use crate::options::EntryOptions;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

/// The serialization format of a [`LoaderFile`], picked from its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    /// `.yml` / `.yaml`
    Yaml,
    /// `.json`
    Json,
}

/// The parsed content of one config file: the entry list plus any unknown
/// top-level keys, which are preserved on write-back.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Document {
    /// The entry tree serialized as a list, in file order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<EntryOptions>,
    /// Unknown top-level keys, round-tripped untouched.
    #[serde(flatten, default, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Node>,
}

impl Document {
    /// A document holding just the given entries.
    pub fn with_entries(entries: Vec<EntryOptions>) -> Self {
        Self {
            entries,
            extra: IndexMap::new(),
        }
    }
}

/// Shared state behind a [`LoaderFile`] handle.
struct FileInner {
    path: PathBuf,
    format: FileFormat,
    suspend: Mutex<usize>,
    /// Serializes whole write sequences (serialize → tmp write → rename).
    /// Without it two writers race on the same sibling `.tmp` file and the
    /// rename can land torn content; see `write_document`.
    write_lock: Mutex<()>,
    deferred: Mutex<DeferredState>,
    deferred_signal: Condvar,
    flusher: Mutex<Option<std::thread::JoinHandle<()>>>,
}

/// State of the coalescing writer behind [`LoaderFile::write_deferred`].
#[derive(Default)]
struct DeferredState {
    /// Latest queued document and its flush deadline.
    pending: Option<(Document, Instant)>,
    /// Monotonic counters so [`LoaderFile::flush_deferred`] can await a
    /// specific queue state.
    queued: u64,
    flushed: u64,
    /// A flush is currently running outside the lock.
    writing: bool,
    /// The owning file is gone; the flusher exits.
    closed: bool,
    /// The last flush error, surfaced through
    /// [`LoaderFile::last_deferred_error`].
    last_error: Option<String>,
}

/// A handle to one config file on disk.
///
/// Handles are cheap to clone and share path, format, and suspend state, so
/// several trees (or the loader and the watcher) can coordinate writes
/// through the same file. While any [`FileSuspendGuard`] is held,
/// [`LoaderFile::write`] is a silent no-op — the file-level half of breaking
/// the write → watch → write feedback loop.
#[derive(Clone)]
pub struct LoaderFile {
    inner: Arc<FileInner>,
}

impl std::fmt::Debug for LoaderFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoaderFile")
            .field("path", &self.inner.path)
            .field("format", &self.inner.format)
            .finish_non_exhaustive()
    }
}

impl LoaderFile {
    /// Open a `.yml`, `.yaml`, or `.json` config file. The file does not
    /// have to exist yet; [`LoaderFile::read`] returns an empty document
    /// until the first write creates it.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let format = match path.extension().and_then(|ext| ext.to_str()) {
            Some("yml" | "yaml") => FileFormat::Yaml,
            Some("json") => FileFormat::Json,
            _ => return Err(IncludeError::UnknownFormat { path }),
        };
        Ok(Self {
            inner: Arc::new(FileInner {
                path,
                format,
                suspend: Mutex::new(0),
                write_lock: Mutex::new(()),
                deferred: Mutex::new(DeferredState::default()),
                deferred_signal: Condvar::new(),
                flusher: Mutex::new(None),
            }),
        })
    }

    /// The file path.
    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// The detected format.
    pub fn format(&self) -> FileFormat {
        self.inner.format
    }

    /// Read and parse the file. Missing and empty files yield an empty
    /// document; `${{ ... }}` templates are *not* expanded here — entries
    /// keep their raw config.
    pub fn read(&self) -> Result<Document> {
        let content = match fs::read_to_string(&self.inner.path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Document::default());
            }
            Err(error) => return Err(error.into()),
        };
        if content.trim().is_empty() {
            return Ok(Document::default());
        }
        match self.inner.format {
            FileFormat::Yaml => {
                let value =
                    serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&content).map_err(|error| {
                        IncludeError::Parse {
                            format: "yaml",
                            source: Box::new(error),
                        }
                    })?;
                if value.is_null() {
                    return Ok(Document::default());
                }
                serde_yaml_ng::from_value(value).map_err(|error| IncludeError::Parse {
                    format: "yaml",
                    source: Box::new(error),
                })
            }
            FileFormat::Json => {
                let value =
                    serde_json::from_str::<serde_json::Value>(&content).map_err(|error| {
                        IncludeError::Parse {
                            format: "json",
                            source: Box::new(error),
                        }
                    })?;
                if value.is_null() {
                    return Ok(Document::default());
                }
                serde_json::from_value(value).map_err(|error| IncludeError::Parse {
                    format: "json",
                    source: Box::new(error),
                })
            }
        }
    }

    /// Serialize the document and replace the file atomically (write to a
    /// sibling `.tmp` file, fsync, rename). A no-op while suspended.
    pub fn write(&self, document: &Document) -> Result<()> {
        write_document(&self.inner, document)
    }

    /// Schedule a coalesced write: rapid calls replace the pending document
    /// (latest wins), and the physical write happens once `delay` has passed
    /// without a newer call. A suspension active at flush time postpones the
    /// write until it lifts. Errors surface through
    /// [`LoaderFile::last_deferred_error`] and never propagate to callers.
    ///
    /// If the flusher thread cannot be spawned, the write happens
    /// synchronously instead.
    pub fn write_deferred(&self, document: Document, delay: Duration) {
        {
            let mut flusher = crate::lock(&self.inner.flusher);
            if flusher.is_none() {
                let weak = Arc::downgrade(&self.inner);
                match std::thread::Builder::new()
                    .name(format!("cordis-flush-{}", self.inner.path.display()))
                    .spawn(move || flusher_loop(weak))
                {
                    Ok(thread) => *flusher = Some(thread),
                    Err(_) => {
                        drop(flusher);
                        let _ = self.write(&document);
                        return;
                    }
                }
            }
        }
        {
            let mut state = crate::lock(&self.inner.deferred);
            state.pending = Some((document, Instant::now() + delay));
            state.queued += 1;
        }
        self.inner.deferred_signal.notify_all();
    }

    /// Block until every [`LoaderFile::write_deferred`] call made before
    /// this one has been flushed (or skipped by suspension lifting and a
    /// later flush).
    pub fn flush_deferred(&self) {
        let mut state = crate::lock(&self.inner.deferred);
        let target = state.queued;
        while state.flushed < target || state.writing || state.pending.is_some() {
            let (guard, _) = self
                .inner
                .deferred_signal
                .wait_timeout(state, Duration::from_millis(100))
                .unwrap_or_else(|error| error.into_inner());
            state = guard;
            if state.flushed >= target && !state.writing && state.pending.is_none() {
                return;
            }
        }
    }

    /// The last deferred-write error, if the flusher failed.
    pub fn last_deferred_error(&self) -> Option<String> {
        crate::lock(&self.inner.deferred).last_error.clone()
    }

    /// Increment the suspend counter, returning a guard whose drop resumes
    /// writes. Hold this while reloading a file so the resulting tree
    /// patches are not written back.
    pub fn suspend(&self) -> FileSuspendGuard {
        {
            let mut suspend = lock(&self.inner.suspend);
            *suspend += 1;
        }
        FileSuspendGuard { file: self.clone() }
    }

    /// Whether any suspend guard is currently held for this file.
    pub fn is_suspended(&self) -> bool {
        *lock(&self.inner.suspend) > 0
    }
}

/// Serialize `document` and atomically replace the file behind `inner`.
/// A no-op while the file is suspended.
///
/// The whole sequence — serialize, write the sibling `.tmp`, fsync, rename —
/// runs under `write_lock`: concurrent writers (a synchronous `write`, the
/// deferred flusher, the loader's write-back) would otherwise interleave on
/// the same `.tmp` path and the rename could publish torn content.
fn write_document(inner: &FileInner, document: &Document) -> Result<()> {
    let _write = crate::lock(&inner.write_lock);
    if *crate::lock(&inner.suspend) > 0 {
        return Ok(());
    }
    if let Ok(metadata) = fs::metadata(&inner.path) {
        if metadata.permissions().readonly() {
            return Err(IncludeError::ReadOnly {
                path: inner.path.clone(),
            });
        }
    }
    let content = match inner.format {
        FileFormat::Yaml => {
            serde_yaml_ng::to_string(document).map_err(|error| IncludeError::Parse {
                format: "yaml",
                source: Box::new(error),
            })?
        }
        FileFormat::Json => {
            let mut text =
                serde_json::to_string_pretty(document).map_err(|error| IncludeError::Parse {
                    format: "json",
                    source: Box::new(error),
                })?;
            text.push('\n');
            text
        }
    };
    if let Some(parent) = inner.path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let file_name = inner
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = inner.path.with_file_name(format!("{file_name}.tmp"));
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&tmp, &inner.path)?;
    Ok(())
}

/// The flusher thread: holds only a weak reference so it dies with the last
/// handle, and loops until the state is closed.
fn flusher_loop(weak: Weak<FileInner>) {
    const SUSPEND_RETRY: Duration = Duration::from_millis(50);
    while let Some(inner) = weak.upgrade() {
        let state = crate::lock(&inner.deferred);
        if state.closed {
            return;
        }
        let deadline = match state.pending.as_ref() {
            Some((_, deadline)) => *deadline,
            None => {
                let waited = inner
                    .deferred_signal
                    .wait(state)
                    .unwrap_or_else(|error| error.into_inner());
                drop(waited);
                continue;
            }
        };
        drop(state);
        let now = Instant::now();
        if now < deadline {
            // Park until the deadline (or a newer document/close) and
            // re-decide with fresh state.
            let state = crate::lock(&inner.deferred);
            let (guard, _) = inner
                .deferred_signal
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|error| error.into_inner());
            drop(guard);
            continue;
        }
        let mut state = crate::lock(&inner.deferred);
        if state.closed {
            return;
        }
        let Some((document, deadline)) = state.pending.take() else {
            continue;
        };
        if Instant::now() < deadline {
            state.pending = Some((document, deadline));
            drop(state);
            continue;
        }
        let queued_at_take = state.queued;
        state.writing = true;
        drop(state);

        let suspended = *crate::lock(&inner.suspend) > 0;
        if suspended {
            let mut state = crate::lock(&inner.deferred);
            state.pending = Some((document, Instant::now() + SUSPEND_RETRY));
            state.writing = false;
            drop(state);
            inner.deferred_signal.notify_all();
            continue;
        }
        let result = write_document(&inner, &document);
        let mut state = crate::lock(&inner.deferred);
        state.writing = false;
        state.flushed = state.flushed.max(queued_at_take);
        if let Err(error) = result {
            state.last_error = Some(error.to_string());
        }
        drop(state);
        inner.deferred_signal.notify_all();
    }
}

impl Drop for FileInner {
    fn drop(&mut self) {
        {
            let mut state = crate::lock(&self.deferred);
            state.closed = true;
        }
        self.deferred_signal.notify_all();
        if let Some(thread) = self
            .flusher
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            let _ = thread.join();
        }
        // Anything still pending (e.g. suspended at close time) is written
        // synchronously so the last state is not lost.
        if let Some((document, _)) = self
            .deferred
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pending
            .take()
        {
            let _ = write_document(self, &document);
        }
    }
}

/// RAII guard for the file-level suspend counter.
#[derive(Debug)]
pub struct FileSuspendGuard {
    file: LoaderFile,
}

impl Drop for FileSuspendGuard {
    fn drop(&mut self) {
        let mut suspend = lock(&self.file.inner.suspend);
        *suspend = suspend.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_file(stem: &str) -> LoaderFile {
        let path = std::env::temp_dir().join(format!(
            "cordis-include-write-test-{stem}-{}-{}.yml",
            std::process::id(),
            randomish()
        ));
        let _ = std::fs::remove_file(&path);
        LoaderFile::open(path).unwrap()
    }

    fn randomish() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as u64)
            .unwrap_or(0)
    }

    fn padded_document(tag: u64) -> Document {
        // A large payload widens the torn-write window.
        let filler = "x".repeat(4096);
        let config = Node::String(format!("{tag}-{filler}"));
        Document::with_entries(vec![
            EntryOptions::new("w").with_id("w").with_config(config),
        ])
    }

    /// Regression: two writers racing on the shared sibling `.tmp` could
    /// interleave and the rename published torn content. The write lock
    /// serializes whole write sequences; the final file must always parse
    /// and equal one of the writers' documents in full.
    #[test]
    fn concurrent_writers_never_produce_a_torn_file() {
        for _ in 0..25 {
            let file = temp_file("race");
            let writers: Vec<_> = (0..4_u64)
                .map(|tag| {
                    let file = file.clone();
                    std::thread::spawn(move || {
                        let document = padded_document(tag);
                        for _ in 0..5 {
                            file.write(&document).unwrap();
                        }
                    })
                })
                .collect();
            for writer in writers {
                writer.join().unwrap();
            }
            let document = file.read().unwrap();
            let config = document.entries[0].config.clone().unwrap();
            let Node::String(text) = &config else {
                panic!("config is not the string one writer wrote: {config:?}");
            };
            let tag: u64 = text.split('-').next().unwrap().parse().unwrap();
            let expected = padded_document(tag);
            assert_eq!(document, expected, "torn or mixed content survived");
            let _ = std::fs::remove_file(file.path());
        }
    }

    /// The deferred flusher and a synchronous writer share the same file;
    /// interleaving them must never leave an unparseable result behind.
    #[test]
    fn concurrent_deferred_and_sync_writes_stay_parseable() {
        for _ in 0..25 {
            let file = temp_file("deferred-race");
            let deferred = std::thread::spawn({
                let file = file.clone();
                move || {
                    for tag in 0..20_u64 {
                        file.write_deferred(padded_document(tag), Duration::from_millis(1));
                    }
                    file.flush_deferred();
                }
            });
            for tag in 100..120_u64 {
                file.write(&padded_document(tag)).unwrap();
            }
            deferred.join().unwrap();
            let document = file.read().unwrap();
            let config = document.entries[0].config.clone().unwrap();
            let Node::String(text) = &config else {
                panic!("config is not the string one writer wrote: {config:?}");
            };
            let tag: u64 = text.split('-').next().unwrap().parse().unwrap();
            assert_eq!(document, padded_document(tag));
            let _ = std::fs::remove_file(file.path());
        }
    }
}
