//! File watching (behind the `watch` feature).
//!
//! Provides the building block for config hot reload: a debounced watcher
//! for one [`LoaderFile`] that skips events produced while the file is
//! suspended. Wiring events into tree reloads is the consumer's job —
//! `cordis-cli` does it for the whole file registry.

use crate::error::{IncludeError, Result};
use crate::file::LoaderFile;
use notify::{Config as NotifyConfig, RecursiveMode, Watcher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

/// A running watch on one config file. Dropping it stops the watcher.
///
/// The callback fires after a short debounce window once the file changes
/// on disk, but never while [`LoaderFile::is_suspended`] holds — writes
/// made through the loader's own suspend guards do not echo back.
#[derive(Debug)]
pub struct FileWatcher {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl LoaderFile {
    /// Watch this file for changes and call `on_change` after the debounce
    /// window, skipping changes observed while the file is suspended.
    ///
    /// The file's *parent directory* is watched (non-recursively) so the
    /// atomic rename performed by [`LoaderFile::write`] is observed
    /// reliably.
    pub fn watch<F>(&self, on_change: F) -> Result<FileWatcher>
    where
        F: Fn() + Send + Sync + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let mut watcher =
            notify::RecommendedWatcher::new(tx, NotifyConfig::default()).map_err(|error| {
                IncludeError::Watch {
                    source: Box::new(error),
                }
            })?;
        let watched = match self.path().parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => self.path().to_path_buf(),
        };
        watcher
            .watch(&watched, RecursiveMode::NonRecursive)
            .map_err(|error| IncludeError::Watch {
                source: Box::new(error),
            })?;

        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let file = self.clone();
        let relevant_paths = relevant_paths(self.path());
        let thread = std::thread::Builder::new()
            .name(format!("cordis-watch-{}", self.path().display()))
            .spawn(move || {
                let _watcher = watcher; // keep the watch alive for the thread's lifetime
                let mut pending = false;
                loop {
                    if stop_flag.load(Ordering::Relaxed) {
                        break;
                    }
                    match rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(Ok(event)) => {
                            // macOS FSEvents reports realpaths, so accept the
                            // canonicalized form of the file as well (e.g.
                            // /tmp/x.yml vs /private/tmp/x.yml). Only the file
                            // itself matches: sibling files in the watched
                            // directory must not trigger a reload.
                            let relevant = event
                                .paths
                                .iter()
                                .any(|event_path| relevant_paths.contains(event_path));
                            if relevant {
                                pending = true;
                            }
                        }
                        Ok(Err(_)) => {}
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if pending {
                                pending = false;
                                if !file.is_suspended() {
                                    on_change();
                                }
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })?;
        Ok(FileWatcher {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for FileWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Paths whose events are relevant: the file and its canonicalized form.
/// Symlinked prefixes (macOS `/tmp`) make backends report paths that differ
/// textually from the configured one.
fn relevant_paths(file: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut candidates = vec![file.to_path_buf()];
    if let Ok(canonical) = std::fs::canonicalize(file) {
        candidates.push(canonical);
    } else if let (Some(name), Some(parent)) = (file.file_name(), file.parent()) {
        // The file may not exist yet; canonicalize the parent instead.
        if let Ok(canonical_parent) = std::fs::canonicalize(parent) {
            candidates.push(canonical_parent.join(name));
        }
    }
    candidates
}
