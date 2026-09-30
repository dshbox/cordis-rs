//! Failure boundary for framework-owned spawned tasks.
//!
//! This is deliberately distinct from [`crate::contained`]. User-code
//! containment catches a panic and converts or discards it according to the
//! surrounding operation. A framework invariant panic is not recoverable: this
//! boundary only adds Runtime logger context, then resumes the same unwind so
//! the task still terminates as panicked. No lifecycle state is repaired here.

use crate::logger::Logger;
use futures::FutureExt;
use std::future::Future;
use std::panic::AssertUnwindSafe;

/// Runtime logger context for a framework task that may panic with nobody
/// left to receive the unwind.
pub(crate) struct PanicReport {
    pub(crate) logger: Logger,
    pub(crate) operation: &'static str,
    pub(crate) context: String,
}

impl PanicReport {
    /// Log one caught framework-task panic; the caller resumes the unwind.
    pub(crate) fn report(&self, payload: &Box<dyn std::any::Any + Send>) {
        self.logger.error(format!(
            "cordis: framework task {} panicked ({}): {}",
            self.operation,
            self.context,
            crate::contained::payload_text(payload)
        ));
    }
}

/// Add diagnostics while preserving a framework invariant panic as a panic.
pub(crate) async fn report_panic<F>(
    logger: Logger,
    operation: &'static str,
    context: String,
    future: F,
) -> F::Output
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    match AssertUnwindSafe(future).catch_unwind().await {
        Ok(output) => output,
        Err(payload) => {
            PanicReport {
                logger,
                operation,
                context,
            }
            .report(&payload);
            std::panic::resume_unwind(payload);
        }
    }
}
