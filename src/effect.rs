//! Lifecycle-owned effects and single-shot asynchronous disposers.

use crate::fiber::{Fiber, FiberInner};
use crate::utils::{block_on, lock, BoxFuture};
use crate::{CordisError, ErrorCode, Result};
use std::fmt::{self, Debug, Formatter};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

/// A boxed, single-shot asynchronous cleanup operation.
pub struct AsyncDisposer {
    callback: Option<Box<dyn FnOnce() -> BoxFuture<Result<()>> + Send + 'static>>,
}

impl AsyncDisposer {
    /// Wrap a synchronous cleanup callback.
    pub fn from_sync<F>(callback: F) -> Self
    where
        F: FnOnce() -> Result<()> + Send + 'static,
    {
        Self {
            callback: Some(Box::new(move || Box::pin(async move { callback() }))),
        }
    }

    /// Wrap an infallible synchronous cleanup callback.
    pub fn infallible<F>(callback: F) -> Self
    where
        F: FnOnce() + Send + 'static,
    {
        Self::from_sync(move || {
            callback();
            Ok(())
        })
    }

    /// Wrap an asynchronous cleanup callback.
    pub fn from_async<F, Fut>(callback: F) -> Self
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        Self {
            callback: Some(Box::new(move || Box::pin(callback()))),
        }
    }

    /// Run this disposer. Calling `run` consumes it, enforcing single-shot use.
    pub async fn run(mut self) -> Result<()> {
        match self.callback.take() {
            Some(callback) => callback().await,
            None => Ok(()),
        }
    }
}

impl Debug for AsyncDisposer {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncDisposer")
            .field("pending", &self.callback.is_some())
            .finish()
    }
}

/// Diagnostic tree describing a live effect and nested effects it owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectMeta {
    /// Human-readable effect label.
    pub label: String,
    /// Nested effect metadata.
    pub children: Vec<EffectMeta>,
}

impl EffectMeta {
    /// Construct a leaf effect metadata node.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            children: Vec::new(),
        }
    }
}

pub(crate) struct EffectCell {
    pub(crate) id: u64,
    owner: Weak<FiberInner>,
    disposed: AtomicBool,
    disposer: Mutex<Option<AsyncDisposer>>,
    children: Mutex<Vec<Arc<EffectCell>>>,
    meta: Mutex<EffectMeta>,
}

impl EffectCell {
    pub(crate) fn new(
        id: u64,
        owner: Weak<FiberInner>,
        label: impl Into<String>,
        disposer: AsyncDisposer,
    ) -> Arc<Self> {
        Arc::new(Self {
            id,
            owner,
            disposed: AtomicBool::new(false),
            disposer: Mutex::new(Some(disposer)),
            children: Mutex::new(Vec::new()),
            meta: Mutex::new(EffectMeta::new(label)),
        })
    }

    async fn dispose(self: &Arc<Self>) -> Result<()> {
        if self.disposed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }

        if let Some(owner) = self.owner.upgrade() {
            owner.remove_effect(self.id);
        }

        let children = {
            let mut children = lock(&self.children);
            std::mem::take(&mut *children)
        };
        let mut first_error = None;
        for child in children.into_iter().rev() {
            if let Err(error) = Box::pin(child.dispose()).await {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }

        let disposer = lock(&self.disposer).take();
        if let Some(disposer) = disposer {
            if let Err(error) = disposer.run().await {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    pub(crate) fn cancel(&self) {
        if self.disposed.swap(true, Ordering::AcqRel) {
            return;
        }
        lock(&self.disposer).take();
        lock(&self.children).clear();
        if let Some(owner) = self.owner.upgrade() {
            owner.remove_effect(self.id);
        }
    }

    fn adopt(self: &Arc<Self>, child: Arc<EffectCell>) -> Result<()> {
        if self.disposed.load(Ordering::Acquire) {
            return Err(CordisError::new(ErrorCode::InactiveEffect));
        }
        if child.disposed.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Some(owner) = child.owner.upgrade() {
            owner.remove_effect(child.id);
        }
        lock(&self.meta).children.push(lock(&child.meta).clone());
        lock(&self.children).push(child);
        Ok(())
    }
}

/// A cloneable handle to one registered effect.
///
/// Dropping a handle does not dispose the effect: ownership belongs to the
/// fiber.  Call [`EffectHandle::dispose`] for early cleanup, or dispose the
/// owning fiber.
#[derive(Clone)]
pub struct EffectHandle {
    pub(crate) cell: Arc<EffectCell>,
}

impl EffectHandle {
    pub(crate) fn new(cell: Arc<EffectCell>) -> Self {
        Self { cell }
    }

    /// Dispose this effect synchronously, waiting for asynchronous cleanup.
    pub fn dispose(&self) -> Result<()> {
        block_on(self.dispose_async())
    }

    /// Dispose this effect asynchronously.
    pub async fn dispose_async(&self) -> Result<()> {
        self.cell.dispose().await
    }

    /// Stop owning this effect without running its cleanup callback.
    ///
    /// This is intended for framework structural effects. Application code
    /// normally wants [`dispose`](Self::dispose).
    pub fn cancel(&self) {
        self.cell.cancel();
    }

    /// Move `child` under this effect's diagnostic and disposal tree.
    pub fn adopt(&self, child: EffectHandle) -> Result<()> {
        self.cell.adopt(child.cell)
    }

    /// Return a snapshot of diagnostic metadata.
    pub fn meta(&self) -> EffectMeta {
        lock(&self.cell.meta).clone()
    }

    /// Whether cleanup has already started.
    pub fn is_disposed(&self) -> bool {
        self.cell.disposed.load(Ordering::Acquire)
    }

    /// Return the owning fiber while it remains alive.
    pub fn owner(&self) -> Option<Fiber> {
        self.cell.owner.upgrade().map(Fiber::from_inner)
    }
}

impl Debug for EffectHandle {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("EffectHandle")
            .field("id", &self.cell.id)
            .field("meta", &self.meta())
            .field("disposed", &self.is_disposed())
            .finish()
    }
}
