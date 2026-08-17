//! Config entry trees and loader files for the
//! [cordis-rs](https://crates.io/crates/cordis-rs) plugin framework.
//!
//! This crate is the data half of porting upstream Cordis' loader: it maps
//! between config files on disk and an in-memory tree of
//! [`Entry`] nodes, each described by [`EntryOptions`]. It deliberately
//! knows nothing about *where plugins come from* and never starts or stops
//! fibers — that is `cordis-loader`'s job, plugged in through the
//! [`PluginResolver`] trait defined here.
//!
//! # Example
//!
//! ```no_run
//! use cordis_include::{Document, EntryOptions, EntryTree, LoaderFile};
//!
//! # fn main() -> cordis_include::Result<()> {
//! let file = LoaderFile::open("cordis.yml")?;
//! let mut document = file.read()?;
//!
//! let tree = EntryTree::new();
//! let diff = tree.update(document.entries)?;
//! for entry in &diff.created {
//!     println!("new entry {} ({})", entry.path(), entry.name());
//! }
//!
//! // Persist generated ids and later edits back to the file.
//! document.entries = tree.serialize();
//! file.write(&document)?;
//! # Ok(())
//! # }
//! ```
//!
//! # File format
//!
//! A file holds an ordered entry list; nested `group` arrays make groups.
//! Object key order is preserved on round-trip, entry fields serialize as
//! `id`, `name`, `disabled`, `inject`, `group`, `config` (config last), and
//! unknown top-level keys are kept untouched — files stay diff-friendly.
//!
//! ```yaml
//! entries:
//!   - id: sched
//!     name: group
//!     group:
//!       - name: adapter-http
//!         config:
//!           port: 8080
//!           host: ${{ env.HOST }}
//! ```
//!
//! `${{ env.NAME }}` templates substitute environment variables when config
//! is handed to a plugin ([`Entry::resolved_config`]); the file itself keeps
//! the template text. There is no expression evaluation.
//!
//! # Suspension
//!
//! Two suspend counters break the reload feedback loop: a file-level guard
//! ([`LoaderFile::suspend`]) suppresses physical writes, and an entry-level
//! guard ([`Entry::suspend`]) tells the loader that an entry's changes came
//! from the file and must not be written back. The `watch` feature adds
//! [`FileWatcher`], a debounced watcher that skips events observed while
//! the file is suspended.
//!
//! # Not in scope
//!
//! Plugin resolution beyond the [`PluginResolver`] contract (static
//! registries and dynamic libraries live in `cordis-loader`), fiber
//! lifecycle, and cascading group semantics (`cordis-group`).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod entry;
pub mod error;
pub mod file;
pub mod interpolate;
pub mod node;
pub mod options;
pub mod resolver;
pub mod tree;
#[cfg(feature = "watch")]
pub mod watch;

pub use entry::{Entry, EntrySuspendGuard};
pub use error::{IncludeError, Result};
pub use file::{Document, FileFormat, FileSuspendGuard, LoaderFile};
pub use node::{Node, NodeMap};
pub use options::{EntryOptions, IMPORT_NAME};
pub use resolver::PluginResolver;
pub use tree::{EntryTree, RemovedEntry, TreeDiff};
#[cfg(feature = "watch")]
pub use watch::FileWatcher;

/// Lock a mutex tolerantly, treating a poisoned lock as unlocked.
///
/// Mirrors the pattern used inside `cordis-rs`: a panic in one thread must
/// not cascade into `unwrap` failures elsewhere. The guarded state may be
/// mid-update, which is acceptable for config trees.
pub(crate) fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}
