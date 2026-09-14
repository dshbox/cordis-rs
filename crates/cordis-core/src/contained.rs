//! Panic containment — the single named boundary around user code that has
//! no caller to propagate to (ADR 0004).
//!
//! Framework wrappers invoke user code in places where a panic has nowhere
//! to go: effect cleanups during a generation drain, a fiber-bound task's
//! drain-join, lifecycle notifications without a caller, log exporters
//! mid-fan-out. The discipline is the same every time — catch the unwind,
//! render the payload, report, continue with the remaining work (upstream's
//! `_unload` disposes through `Promise.all` and logs each failure,
//! fiber.ts:437-447). This module owns the reusable containment/reporting
//! helpers for framework-owned work with no caller. Operation-specific adapters
//! such as Event dispatch and update control also contain user callbacks locally
//! when they must cover both synchronous callback construction and asynchronous
//! polling in one semantic boundary. Two reusable policies live here:
//!
//! - [`contain_join`] recovers a panicked task through its join handle and
//!   reports it while allowing the drain to continue;
//!   recovering a panicked task's payload from the handle's error arm;
//! - [`catch_contained`] — catch and hand the payload back, for wrappers
//!   whose policy is "convert the panic into a failure" (a panicking
//!   plugin apply becomes the Failed state; a panicking effect cleanup
//!   becomes a `Panic`-kind failure). The payload renders through
//!   the one [`payload_text`] either way.
//!
//! The report routes through the given [`Logger`] when the site has one —
//! attached exporters see contained panics as warn records — and falls
//! back to stderr otherwise. Pass `None` deliberately when reporting
//! through a logger would re-enter the contained surface: the logger's
//! own exporter dispatch is the canonical case (an always-panicking
//! exporter would turn the report into unbounded recursion), so dispatch
//! itself reports via stderr.
//!
//! This module is framework code and holds no locks. The *contained*
//! closure or future is user code: callers keep the closed-critical-section
//! law (ADR 0010) — containment never widens a critical section.
//!
//! [`Logger`]: crate::logger::Logger

use crate::logger::Logger;
use futures::FutureExt;
use std::future::Future;

/// Catch-and-convert containment: the caught
/// payload is handed to the caller instead of being reported here — the
/// wrapper turns it into its own failure channel (e.g. the Failed fiber
/// state). Operation-specific callback adapters may own an equivalent local
/// boundary when their semantic failure type must cover synchronous construction
/// and asynchronous polling together.
pub(crate) async fn catch_contained<F>(
    future: F,
) -> Result<F::Output, Box<dyn std::any::Any + Send>>
where
    F: Future,
{
    // The future may borrow state the
    // panic leaves inconsistent; containing beats refusing
    std::panic::AssertUnwindSafe(future).catch_unwind().await
}

/// The one copy of the payload rendering: `&str`/`String` payloads render
/// verbatim, anything else degrades to a fixed marker.
pub(crate) fn payload_text(payload: &Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_owned())
}

/// Run one user callback, containing its panic — the [module
/// docs](self) spell out the policy and the report routing.
///
/// A panicking callback is reported (logger or stderr) and discarded;
/// execution continues with the caller's next work. The callback's own
/// return value is dropped on panic — sites that need an outcome should
/// deliver it through their own channel, not a return type.
pub(crate) fn contain<F>(what: &'static str, logger: Option<&Logger>, callback: F)
where
    F: FnOnce(),
{
    // AssertUnwindSafe: the alternative is refusing to contain callbacks
    // that borrow shared state — exactly the callbacks that need it. State
    // possibly left inconsistent by the panic is why the policy is "report
    // and continue", never "retry".
    if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback)) {
        report(what, logger, &payload);
    }
}

/// Recover and report a panic from a spawned task's join handle.
/// A task that
/// panicked does not panic the join — tokio catches it at the task
/// boundary, so the handle resolves `Ok(Err(err))` with the payload
/// inside the error — and that arm is recovered and reported here,
/// making the drain-join's "contained and reported" contract
/// (`Context::run`'s ordering-trick doc) observable. The cancellation
/// arm stays silent: nothing in-tree aborts the handle, and a runtime
/// shutdown cancels the joining cleanup with it.
pub(crate) async fn contain_join(
    what: &'static str,
    logger: Option<&Logger>,
    join: tokio::task::JoinHandle<()>,
) {
    match std::panic::AssertUnwindSafe(join).catch_unwind().await {
        // a panic on the join's own poll: same posture as contain_async
        Err(payload) => report(what, logger, &payload),
        Ok(Ok(())) => {}
        Ok(Err(join_error)) if join_error.is_panic() => {
            report(what, logger, &join_error.into_panic());
        }
        Ok(Err(_cancelled)) => {}
    }
}

/// The one copy of the report routing: a rendered line goes through the
/// logger's exporters as a warn record when a logger is given, stderr
/// otherwise (the module docs name the deliberate `None` sites). Panic
/// reports and already-normalized failure reports share this routing —
/// one `cordis:`-prefixed format for both, so an operator greps the
/// prefix without caring where the report landed.
pub(crate) fn report_text(logger: Option<&Logger>, text: String) {
    match logger {
        Some(logger) => logger.warn(text),
        None => eprintln!("{text}"),
    }
}

/// Panic reports: render the payload, then the shared routing.
fn report(what: &'static str, logger: Option<&Logger>, payload: &Box<dyn std::any::Any + Send>) {
    report_text(
        logger,
        format!("cordis: {what} panicked: {}", payload_text(payload)),
    );
}

/// Silence the default panic hook for the duration of `f` — tests that
/// deliberately panic inside containment use this so the contained panic
/// does not print a backtrace between test output.
#[cfg(test)]
pub(crate) fn quiet_hook<T>(f: impl FnOnce() -> T) -> T {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let out = f();
    std::panic::set_hook(default);
    out
}

#[cfg(test)]
mod tests {
    // The containment policy's one test home: payload rendering, logger
    // routing, the stderr fallback, and the async arm are asserted HERE,
    // once; the contained sites (fiber drain, drain-join, log dispatch)
    // keep only per-site survival smokes in the contract tier.
    use super::{contain, contain_join, quiet_hook};
    use crate::logger::{BufferExporter, Level, Logger, LoggerService};
    use std::sync::Arc;

    fn logger_with_buffer() -> (Logger, Arc<BufferExporter>) {
        let service = LoggerService::new();
        let buffer = Arc::new(BufferExporter::new(16, Level::Debug).unwrap());
        let id = service.reserve_exporter();
        service.insert_reserved(id, buffer.clone());
        // a bare channel: name/level routing is dispatch's business, this
        // set pins the containment policy only
        (Logger::new_for_test(Arc::new(service)), buffer)
    }

    fn texts(exporter: &BufferExporter) -> Vec<String> {
        exporter
            .snapshot()
            .into_iter()
            .map(|record| record.text().to_owned())
            .collect()
    }

    #[test]
    fn sync_panicking_callback_is_contained_and_reaches_exporters() {
        let (logger, buffer) = logger_with_buffer();
        quiet_hook(|| {
            contain("test callback", Some(&logger), || {
                panic!("boom &str");
            })
        });
        // containment: contain returned; routing: the report went through
        // the logger's exporters as a warn (severity checked via the
        // record, not just the text)
        let records = buffer.snapshot();
        assert_eq!(records.len(), 1, "exactly one report per contained panic");
        assert_eq!(records[0].level(), Level::Warn);
        assert_eq!(
            records[0].text(),
            "cordis: test callback panicked: boom &str"
        );
    }

    #[test]
    fn payload_kinds_render_once_each() {
        // &str (panic!), String (panic_any), and non-string payloads are
        // the ritual's whole matrix — pinned here so no site re-derives it
        let (logger, buffer) = logger_with_buffer();
        quiet_hook(|| {
            contain("site", Some(&logger), || panic!("as str"));
            contain("site", Some(&logger), || {
                std::panic::panic_any("as String".to_owned())
            });
            contain("site", Some(&logger), || std::panic::panic_any(7u32));
        });
        assert_eq!(
            texts(&buffer),
            [
                "cordis: site panicked: as str",
                "cordis: site panicked: as String",
                "cordis: site panicked: non-string panic payload",
            ]
        );
    }

    #[test]
    fn sync_without_logger_falls_back_to_stderr() {
        // None is the deliberate no-logger arm (dispatch uses it to avoid
        // re-entering itself): the observable contract is that the panic
        // neither escapes nor derails the caller — the stderr text itself
        // is not capturable without new deps and stays unasserted
        quiet_hook(|| contain("no logger", None, || panic!("quiet boom")));
    }

    #[tokio::test]
    async fn panicked_task_is_reported_through_the_join_handle() {
        // the recovery arm contain_join exists for: tokio catches the
        // task's panic at its boundary and the handle resolves
        // Ok(Err(JoinError)) — the payload inside the error is reported,
        // not dropped (the drain-join contract's "reported" half)
        let (logger, buffer) = logger_with_buffer();
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let join = tokio::spawn(async {
            panic!("join boom");
        });
        contain_join("join site", Some(&logger), join).await;
        std::panic::set_hook(default);
        assert_eq!(texts(&buffer), ["cordis: join site panicked: join boom"]);
    }
}
