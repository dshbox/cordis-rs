//! Error type for the include layer.

use std::error::Error as StdError;
use std::fmt;
use std::io;
use std::path::PathBuf;

/// Result alias used throughout `cordis-include`.
pub type Result<T> = std::result::Result<T, IncludeError>;

/// Errors produced while reading entry trees and loader files.
#[derive(Debug)]
pub enum IncludeError {
    /// An underlying filesystem operation failed.
    Io(io::Error),
    /// A config file could not be parsed or serialized.
    Parse {
        /// Which format was being processed (`"yaml"` or `"json"`).
        format: &'static str,
        /// The parser or serializer error.
        source: Box<dyn StdError + Send + Sync>,
    },
    /// The file extension does not map to a supported format.
    UnknownFormat {
        /// The path that was opened.
        path: PathBuf,
    },
    /// The target file is read-only, so configuration cannot be written back.
    ReadOnly {
        /// The path that was opened.
        path: PathBuf,
    },
    /// Two entries in one tree declare the same id.
    DuplicateId {
        /// The conflicting id.
        id: String,
    },
    /// No entry with the requested id exists in the tree.
    EntryNotFound {
        /// The requested (possibly composite) id.
        id: String,
    },
    /// An entry id is empty or contains the `:` path separator.
    InvalidId {
        /// The offending id.
        id: String,
    },
    /// An entry has no plugin name.
    InvalidName,
    /// A `${{ env.NAME }}` template referenced an unset variable.
    MissingEnv {
        /// The full expression that failed, e.g. `env.MISSING`.
        expression: String,
    },
    /// A `${{ ... }}` template used an unsupported expression.
    UnknownExpression {
        /// The unsupported expression.
        expression: String,
    },
    /// A `${{` template was never closed with `}}`.
    Unterminated {
        /// The input containing the dangling `${{`.
        input: String,
    },
    /// The referenced entry does not belong to this tree.
    NotInTree,
    /// The requested move would place an entry inside its own subtree.
    Cycle,
    /// Setting up or running a file watcher failed (`watch` feature).
    Watch {
        /// The underlying watcher error.
        source: Box<dyn StdError + Send + Sync>,
    },
}

impl fmt::Display for IncludeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "filesystem error: {error}"),
            Self::Parse { format, source } => write!(f, "{format} parse error: {source}"),
            Self::UnknownFormat { path } => write!(
                f,
                "unsupported config format (expected .yml, .yaml, or .json): {}",
                path.display()
            ),
            Self::ReadOnly { path } => {
                write!(f, "config file is read-only: {}", path.display())
            }
            Self::DuplicateId { id } => write!(f, "duplicate entry id `{id}`"),
            Self::EntryNotFound { id } => write!(f, "entry `{id}` not found"),
            Self::InvalidId { id } => {
                write!(
                    f,
                    "invalid entry id `{id}`: must be non-empty and contain no `:`"
                )
            }
            Self::InvalidName => write!(f, "entry name must be non-empty"),
            Self::MissingEnv { expression } => {
                write!(f, "environment variable unset: `{expression}`")
            }
            Self::UnknownExpression { expression } => {
                write!(f, "unsupported template expression `{expression}`")
            }
            Self::Unterminated { input } => {
                write!(f, "unterminated template `${{{{`}} in `{input}`")
            }
            Self::NotInTree => write!(f, "entry does not belong to this tree"),
            Self::Cycle => write!(f, "cannot move an entry into its own subtree"),
            Self::Watch { source } => write!(f, "file watcher error: {source}"),
        }
    }
}

impl StdError for IncludeError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Parse { source, .. } => Some(source.as_ref()),
            Self::Watch { source } => Some(source.as_ref()),
            _ => None,
        }
    }
}

impl From<io::Error> for IncludeError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
