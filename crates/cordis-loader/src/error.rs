//! Error type combining core and include failures.

use cordis::CordisError;
use cordis_include::IncludeError;
use std::error::Error as StdError;
use std::fmt;

/// Result alias used throughout `cordis-loader`.
pub type Result<T> = std::result::Result<T, LoaderError>;

/// Errors produced while loading entries and driving fibers.
#[derive(Debug)]
pub enum LoaderError {
    /// A core lifecycle or registry operation failed.
    Cordis(CordisError),
    /// The entry tree or config file layer failed.
    Include(IncludeError),
}

impl fmt::Display for LoaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cordis(error) => write!(f, "{error}"),
            Self::Include(error) => write!(f, "{error}"),
        }
    }
}

impl StdError for LoaderError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Cordis(error) => Some(error),
            Self::Include(error) => Some(error),
        }
    }
}

impl From<CordisError> for LoaderError {
    fn from(error: CordisError) -> Self {
        Self::Cordis(error)
    }
}

impl From<IncludeError> for LoaderError {
    fn from(error: IncludeError) -> Self {
        Self::Include(error)
    }
}
