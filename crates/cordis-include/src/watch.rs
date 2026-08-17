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
        let path = self.path().to_path_buf();
        let thread = std::thread::Builder::new()
            .name(format!("cordis-watch-{}", path.display()))
            .spawn(move || {
                let _watcher = watcher; // keep the watch alive for the thread's lifetime
                let mut pending = false;
                loop {
                    if stop_flag.load(Ordering::Relaxed) {
                        break;
                    }
                    match rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(Ok(event)) => {
                            if event.paths.iter().any(|event_path| event_path == &path) {
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
