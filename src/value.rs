//! Cloneable, dynamically typed values used for services, events, and config.

use crate::{CordisError, ErrorCode, Result};
use std::any::{Any, type_name};
use std::fmt::{self, Debug, Formatter};
use std::sync::Arc;

/// A cloneable type-erased `Send + Sync` value.
///
/// Cordis TypeScript stores arbitrary JavaScript values.  This wrapper is the
/// Rust equivalent: callers recover the concrete type through [`Value::downcast`].
#[derive(Clone)]
pub struct Value {
    inner: Arc<dyn Any + Send + Sync>,
    type_name: &'static str,
}

impl Value {
    /// Erase a concrete value.
    pub fn new<T>(value: T) -> Self
    where
        T: Any + Send + Sync,
    {
        Self {
            inner: Arc::new(value),
            type_name: type_name::<T>(),
        }
    }

    /// Erase an existing `Arc` without adding a second `Arc` layer.
    pub fn from_arc<T>(value: Arc<T>) -> Self
    where
        T: Any + Send + Sync,
    {
        Self {
            inner: value,
            type_name: type_name::<T>(),
        }
    }

    /// Return the stored concrete type's diagnostic name.
    pub const fn type_name(&self) -> &'static str {
        self.type_name
    }

    /// Whether this value stores `T`.
    pub fn is<T: Any>(&self) -> bool {
        self.inner.is::<T>()
    }

    /// Recover an `Arc<T>` from the erased value.
    pub fn downcast<T>(&self) -> Result<Arc<T>>
    where
        T: Any + Send + Sync,
    {
        self.inner.clone().downcast::<T>().map_err(|_| {
            CordisError::with_message(
                ErrorCode::TypeMismatch,
                format!(
                    "expected value of type `{}`, found `{}`",
                    type_name::<T>(),
                    self.type_name
                ),
            )
        })
    }

    /// Borrow the underlying `Any` trait object.
    pub fn as_any(&self) -> &(dyn Any + Send + Sync) {
        self.inner.as_ref()
    }
}

impl Default for Value {
    fn default() -> Self {
        Self::new(())
    }
}

impl Debug for Value {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Value")
            .field("type_name", &self.type_name)
            .finish_non_exhaustive()
    }
}

/// Type-erased plugin configuration.
pub type Config = Value;
