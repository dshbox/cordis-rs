//! Framework errors and configuration validation diagnostics.

use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

/// Stable, machine-readable Cordis error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorCode {
    /// An effect was created from a disposed or unloading context.
    InactiveEffect,
    /// A requested service does not exist in the current scope.
    MissingService,
    /// A dynamically stored value had an unexpected concrete Rust type.
    TypeMismatch,
    /// A service or property was registered more than once in one scope.
    DuplicateService,
    /// A service was mutated by a fiber other than its provider.
    AccessDenied,
    /// Plugin configuration was rejected by its validator.
    InvalidConfig,
    /// Plugin startup failed.
    Plugin,
    /// An event listener or middleware failed.
    Event,
    /// A property is already declared using another reflection mode.
    PropertyConflict,
    /// A general framework error.
    Other,
}

impl ErrorCode {
    /// Return the default human-readable message for this code.
    pub const fn message(self) -> &'static str {
        match self {
            Self::InactiveEffect => "cannot create effect on inactive context",
            Self::MissingService => "required service is unavailable",
            Self::TypeMismatch => "stored value has an unexpected type",
            Self::DuplicateService => "service has already been registered",
            Self::AccessDenied => "service belongs to another fiber",
            Self::InvalidConfig => "invalid config",
            Self::Plugin => "plugin failed",
            Self::Event => "event listener failed",
            Self::PropertyConflict => "property is already declared",
            Self::Other => "cordis error",
        }
    }
}

/// An individual standard-schema-style validation issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    /// Human-readable problem description.
    pub message: String,
    /// Path segments locating the invalid value.
    pub path: Vec<String>,
}

impl ValidationIssue {
    /// Construct an issue without a path.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            path: Vec::new(),
        }
    }

    /// Attach a path to this issue.
    pub fn at(mut self, path: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.path = path.into_iter().map(Into::into).collect();
        self
    }
}

/// Aggregated plugin configuration validation error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    /// All issues reported by the validator.
    pub issues: Vec<ValidationIssue>,
}

impl ValidationError {
    /// Construct an aggregate from one or more issues.
    pub fn new(issues: impl IntoIterator<Item = ValidationIssue>) -> Self {
        Self {
            issues: issues.into_iter().collect(),
        }
    }
}

impl Display for ValidationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "invalid config:")?;
        for (index, issue) in self.issues.iter().enumerate() {
            write!(f, "  - {}", issue.message)?;
            if !issue.path.is_empty() {
                write!(f, " (at {})", issue.path.join("."))?;
            }
            if index + 1 < self.issues.len() {
                writeln!(f)?;
            }
        }
        Ok(())
    }
}

impl Error for ValidationError {}

/// Error type used throughout the framework.
#[derive(Debug, Clone)]
pub struct CordisError {
    code: ErrorCode,
    message: String,
    validation: Option<ValidationError>,
    source: Option<Arc<dyn Error + Send + Sync + 'static>>,
}

impl CordisError {
    /// Construct an error with the code's default message.
    pub fn new(code: ErrorCode) -> Self {
        Self {
            code,
            message: code.message().to_owned(),
            validation: None,
            source: None,
        }
    }

    /// Construct an error with a custom message.
    pub fn with_message(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            validation: None,
            source: None,
        }
    }

    /// Construct an error wrapping an underlying cause.
    pub fn with_source(
        code: ErrorCode,
        message: impl Into<String>,
        source: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            validation: None,
            source: Some(Arc::new(source)),
        }
    }

    /// Convert validation issues to a Cordis error.
    pub fn validation(issues: impl IntoIterator<Item = ValidationIssue>) -> Self {
        let validation = ValidationError::new(issues);
        Self {
            code: ErrorCode::InvalidConfig,
            message: validation.to_string(),
            validation: Some(validation),
            source: None,
        }
    }

    /// Return the stable error code.
    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    /// Return validation details when this is an invalid-config error.
    pub fn validation_error(&self) -> Option<&ValidationError> {
        self.validation.as_ref()
    }

    /// Attach context to an existing error while preserving its code.
    pub fn context(mut self, context: impl AsRef<str>) -> Self {
        self.message = format!("{}: {}", context.as_ref(), self.message);
        self
    }

    /// Attach an underlying cause while preserving code and message.
    pub fn caused_by(mut self, source: impl Error + Send + Sync + 'static) -> Self {
        self.source = Some(Arc::new(source));
        self
    }
}

impl Display for CordisError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for CordisError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
    }
}

impl From<ValidationError> for CordisError {
    fn from(value: ValidationError) -> Self {
        Self {
            code: ErrorCode::InvalidConfig,
            message: value.to_string(),
            validation: Some(value),
            source: None,
        }
    }
}

impl From<String> for CordisError {
    fn from(value: String) -> Self {
        Self::with_message(ErrorCode::Other, value)
    }
}

impl From<&str> for CordisError {
    fn from(value: &str) -> Self {
        Self::with_message(ErrorCode::Other, value)
    }
}

/// Framework result alias.
pub type Result<T, E = CordisError> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_chain_survives_construction_and_clone() {
        let cause = std::io::Error::other("disk gone");
        let error = CordisError::with_source(ErrorCode::Plugin, "plugin failed", cause);
        let source = Error::source(&error).expect("source recorded");
        assert_eq!(source.to_string(), "disk gone");
        assert_eq!(error.code(), ErrorCode::Plugin);
        assert_eq!(error.to_string(), "plugin failed");

        let cloned = error.clone();
        assert_eq!(
            Error::source(&cloned).map(ToString::to_string).as_deref(),
            Some("disk gone")
        );

        let attached = CordisError::new(ErrorCode::Event).caused_by(std::io::Error::other("inner"));
        assert_eq!(
            Error::source(&attached).map(ToString::to_string).as_deref(),
            Some("inner")
        );
    }
}
