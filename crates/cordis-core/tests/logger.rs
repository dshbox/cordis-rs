//! Issue 44 executable evidence for immutable LogRecords, semantic Level
//! filtering, exact generation-owned exporter occurrences, recursion/failure
//! isolation, opt-in bounded buffering, and Logger foundation availability.
//! Tests assert assignment and receipt semantics without inventing a global
//! callback ordering guarantee.

mod common;

use common::deadlock_watchdog;
use cordis_core::logger::{BufferExporter, BufferSizeZero, Exporter, LogRecord};
use cordis_core::{Context, Level, Plugin, PreparedPlugin};
use parking_lot::Mutex;
use std::borrow::Cow;
use std::convert::Infallible;
use std::sync::Arc;

/// Counting exporter for assertions.
#[derive(Default)]
struct Counting {
    messages: Mutex<Vec<LogRecord>>,
}

impl Counting {
    fn new() -> Arc<Self> {
        Arc::default()
    }

    fn len(&self) -> usize {
        self.messages.lock().len()
    }
}

impl Exporter for Counting {
    fn export(&self, message: &LogRecord) {
        // clone before the lock (ADR 0010 row 1)
        let record = message.clone();
        self.messages.lock().push(record);
    }
}

/// A plugin that hands its apply context out through a shared slot — the
/// capture shape the lifecycle rows below share (three local variants
/// converged per the review pass). Its CamelCase name doubles as the
/// hyphenate subject: the fiber's default channel is "probe-exporter".
type Captured = Arc<Mutex<Option<Context>>>;

struct ContextGrabber {
    captured: Captured,
}

impl Plugin for ContextGrabber {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("ProbeExporter")
    }
    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), Infallible> {
        *self.captured.lock() = Some(ctx);
        Ok(())
    }
}

#[test]
fn level_filtering_by_exporter_minimum() {
    let ctx = Context::new();
    let counting = Counting::new();

    struct Fixed(Level);
    impl Exporter for Fixed {
        fn export(&self, _m: &LogRecord) {}
        fn default_level(&self) -> Level {
            self.0
        }
    }
    // Warn-level exporter must not see debug messages
    ctx.add_exporter(Arc::new(Fixed(Level::Warn))).unwrap();
    ctx.add_exporter(counting.clone()).unwrap();
    // Counting uses default (Info)

    let logger = ctx.logger();
    logger.debug("invisible at warn");
    logger.info("visible");

    assert_eq!(counting.len(), 1);
}

// Per-name routing (upstream `exporter.levels[name] ?? …`, the
// carried-verbatim half of the level chain): the same Info record passes
// or drops per channel — only the exporter's per-name answer differs.
#[test]
fn min_level_routes_per_channel_name() {
    let ctx = Context::new();
    let counting = Counting::new();

    struct Routing {
        sink: Arc<Counting>,
    }
    impl Exporter for Routing {
        fn export(&self, message: &LogRecord) {
            self.sink.export(message);
        }
        fn min_level(&self, name: &str) -> Option<Level> {
            (name == "audit").then_some(Level::Debug)
        }
        fn default_level(&self) -> Level {
            Level::Warn
        }
    }
    ctx.add_exporter(Arc::new(Routing {
        sink: counting.clone(),
    }))
    .unwrap();

    ctx.logger().with_name("main").info("main info");
    ctx.logger().with_name("audit").info("audit info");
    ctx.logger().with_name("main").warn("main warn");

    let texts: Vec<String> = counting
        .messages
        .lock()
        .iter()
        .map(|m| m.text().to_owned())
        .collect();
    assert_eq!(
        texts,
        ["audit info".to_owned(), "main warn".to_owned()],
        "the default-Warn channel drops its Info; the audit channel's per-name Debug admits it"
    );
}

#[test]
fn buffer_exporter_keeps_last_records() {
    let ctx = Context::new();
    let buffer = Arc::new(BufferExporter::new(3, Level::Info).unwrap());
    ctx.add_exporter(buffer.clone()).unwrap();

    let logger = ctx.logger();
    for i in 0..5 {
        logger.info(format!("msg-{i}"));
    }

    let snapshot = buffer.snapshot();
    assert_eq!(snapshot.len(), 3);
    assert_eq!(snapshot[0].text(), "msg-2");
    assert_eq!(snapshot[2].text(), "msg-4");
}

// The recorded divergence from upstream (upstream-facts §13, "no hidden
// retention"; ADR 0006's deliberately-omitted ledger): upstream installs
// a 1000-entry ring buffer in the LoggerService constructor; the port
// installs nothing. Observable contract: logs emitted before the first
// exporter registration are dropped — no retention happens anywhere, and
// a later-registered exporter does not receive them retroactively. The
// Debug-level buffer rules out level routing as the reason the
// pre-registration record is absent.
#[test]
fn no_default_buffer_exporter_pre_registration_logs_are_dropped() {
    let ctx = Context::new();
    ctx.logger().warn("emitted before any exporter");

    let buffer = Arc::new(BufferExporter::new(8, Level::Debug).unwrap());
    ctx.add_exporter(buffer.clone()).unwrap();

    ctx.logger().warn("emitted after registration");

    let texts: Vec<_> = buffer
        .snapshot()
        .into_iter()
        .map(|m| m.text().to_owned())
        .collect();
    assert_eq!(
        texts,
        ["emitted after registration"],
        "nothing is retained by default and pre-registration logs are not replayed"
    );
}

#[test]
fn named_loggers_are_independent_channels() {
    let ctx = Context::new();
    let counting = Counting::new();
    ctx.add_exporter(counting.clone()).unwrap();

    let root = ctx.logger();
    let custom = root.with_name("my-plugin");
    assert_eq!(custom.name(), "my-plugin");

    custom.warn("from custom");
    assert_eq!(counting.len(), 1);
    assert_eq!(counting.messages.lock()[0].channel(), "my-plugin");
}

// IU-05: the default channel name is the exact hyphenation of the Fiber name.
#[tokio::test]
async fn logger_names_default_to_the_hyphenated_fiber_name() {
    let ctx = Context::new();
    assert_eq!(ctx.logger().name(), "root", "the root fiber's name");

    let captured: Captured = Default::default();
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(
            ContextGrabber {
                captured: captured.clone(),
            },
            (),
        ))
        .await
        .unwrap();
    fiber_handle.ready().await.unwrap();

    let plugin_ctx = captured.lock().clone().expect("apply ran");
    assert_eq!(
        plugin_ctx.logger().name(),
        "probe-exporter",
        "the plugin fiber's name, hyphenated like upstream's cosmokit hyphenate"
    );
}

#[test]
fn sequence_numbers_are_monotonic() {
    let ctx = Context::new();
    let counting = Counting::new();
    ctx.add_exporter(counting.clone()).unwrap();

    let logger = ctx.logger();
    logger.error("a");
    logger.error("b");
    logger.error("c");

    let msgs = counting.messages.lock();
    assert!(msgs[1].sequence() > msgs[0].sequence() && msgs[2].sequence() > msgs[1].sequence());
    assert!(msgs[2].timestamp() >= msgs[0].timestamp());
}

#[tokio::test]
async fn exporter_is_removed_when_its_fiber_disposes() {
    struct ExporterPlugin {
        counting: Arc<Counting>,
    }
    impl Plugin for ExporterPlugin {
        type Config = ();
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = cordis_core::effect::EffectRegistrationError;
        fn name(&self) -> Cow<'static, str> {
            Cow::Borrowed("exporter-plugin")
        }
        fn prepare(&self, (): ()) -> Result<(), Infallible> {
            Ok(())
        }
        async fn apply(
            &self,
            ctx: Context,
            _prepared: &(),
        ) -> Result<(), cordis_core::effect::EffectRegistrationError> {
            ctx.add_exporter(self.counting.clone())?;
            Ok(())
        }
    }

    let ctx = Context::new();
    let counting = Counting::new();
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(
            ExporterPlugin {
                counting: counting.clone(),
            },
            (),
        ))
        .await
        .unwrap();
    fiber_handle.ready().await.unwrap();

    // attached while the plugin is active
    ctx.logger().info("during");
    assert_eq!(counting.len(), 1);

    // upstream registers exporters via ctx.effect (logger.ts
    // `ctx.logger.exporter()`), so dispose detaches them
    fiber_handle.dispose().await.unwrap();
    ctx.logger().info("after");
    assert_eq!(
        counting.len(),
        1,
        "a disposed plugin's exporter must stop receiving messages"
    );
}

// Review 2026-08-27, finding 12 (carried): a zero capacity is rejected
// with a typed error instead of being quietly clamped to 1.
#[test]
fn buffer_exporter_rejects_zero_capacity() {
    let err = BufferExporter::new(0, Level::Info)
        .err()
        .expect("zero capacity is rejected");
    assert!(matches!(err, BufferSizeZero));
    assert_eq!(err.to_string(), "buffer_size must be greater than zero");
}

#[test]
fn level_as_str_uses_the_wire_names() {
    assert_eq!(Level::Error.as_str(), "error");
    assert_eq!(Level::Warn.as_str(), "warn");
    assert_eq!(Level::Info.as_str(), "info");
    assert_eq!(Level::Debug.as_str(), "debug");
}

#[test]
fn buffer_exporter_clear_empties_the_ring() {
    let ctx = Context::new();
    let exporter = Arc::new(BufferExporter::new(8, Level::Debug).unwrap());
    ctx.add_exporter(exporter.clone()).unwrap();

    ctx.logger().error("one");
    ctx.logger().warn("two");
    assert_eq!(exporter.snapshot().len(), 2);

    exporter.clear();
    assert!(exporter.snapshot().is_empty(), "clear empties the ring");
}

// ---------------------------------------------------------------------------
// add_exporter registers as an effect of the current fiber — on a disposed
// fiber the gated push refuses and the just-added exporter must be rolled
// back instead of leaking in the shared LoggerService.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn add_exporter_on_disposed_fiber_errors_and_rolls_back() {
    let root = Context::new();
    let captured: Captured = Default::default();
    let fiber_handle = root
        .spawn(PreparedPlugin::from_input(
            ContextGrabber {
                captured: captured.clone(),
            },
            (),
        ))
        .await
        .unwrap();
    let dead = captured.lock().clone().expect("apply ran");
    fiber_handle.dispose().await.unwrap();

    let err = dead
        .add_exporter(Arc::new(BufferExporter::new(4, Level::Info).unwrap()))
        .expect_err("exporter registration on a disposed fiber must fail");
    assert!(matches!(
        err,
        cordis_core::effect::EffectRegistrationError::InactiveContext
    ));

    // the live root still accepts exporters
    root.add_exporter(Arc::new(BufferExporter::new(4, Level::Info).unwrap()))
        .expect("live context accepts exporters");
}

// ADR 0010 probe (race suite, reentrancy watchdog logger ×1 — ADR 0012
// decision 6; v1 review 2026-08-28 fourth pass): a removed exporter's Arc
// may be the last reference, so its Drop is user code and must not run
// while the exporter-list lock is held. The probe's Drop re-enters the
// list through the public logging API (`error` snapshots the exporter
// list under the same lock); removal happens through the fiber-dispose
// disposer of `Context::add_exporter`.
//
// The dispose future runs the drain (and with it the removal) inline, so
// the probe thread owns the runtime; a regression deadlocks that thread
// while the watchdog's recv timeout turns the hang into a failure.

struct DroppingExporter {
    logger: cordis_core::Logger,
}

impl Exporter for DroppingExporter {
    fn export(&self, _message: &LogRecord) {}
}

impl Drop for DroppingExporter {
    fn drop(&mut self) {
        self.logger.error("dropping exporter");
    }
}

#[tokio::test]
async fn exporter_removal_drops_the_exporter_outside_the_list_lock() {
    let root = Context::new();
    let captured: Captured = Default::default();
    let fiber_handle = root
        .spawn(PreparedPlugin::from_input(
            ContextGrabber {
                captured: captured.clone(),
            },
            (),
        ))
        .await
        .unwrap();
    let scoped = captured.lock().clone().expect("apply ran");

    scoped
        .add_exporter(Arc::new(DroppingExporter {
            logger: root.logger(),
        }))
        .expect("exporter registers on the live fiber");

    deadlock_watchdog(
        "exporter removal deadlocked: the exporter's Drop ran under the list lock",
        move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("probe runtime");
            rt.block_on(fiber_handle.dispose())
                .expect("dispose itself succeeds");
        },
    );
}

// ---------------------------------------------------------------------------
// Exact exporter control is a move-only registration returned by publication.
// ---------------------------------------------------------------------------

#[test]
fn exporter_registration_removes_one_exact_duplicate_occurrence() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Hits(Arc<AtomicUsize>);
    impl Exporter for Hits {
        fn export(&self, _record: &LogRecord) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    let ctx = Context::new();
    let hits = Arc::new(AtomicUsize::new(0));
    let exporter = Arc::new(Hits(hits.clone()));
    let first = ctx.add_exporter(exporter.clone()).unwrap();
    let second = ctx.add_exporter(exporter).unwrap();
    ctx.logger().info("two");
    assert_eq!(hits.load(Ordering::Relaxed), 2);
    assert!(first.remove());
    ctx.logger().info("one");
    assert_eq!(hits.load(Ordering::Relaxed), 3);
    drop(second);
    ctx.logger().info("drop inert");
    assert_eq!(hits.load(Ordering::Relaxed), 4);
}

// ---------------------------------------------------------------------------
// The refusal-no-trace twin (gated-publish, ADR 0015): a refused
// exporter registration leaves nothing in the runtime-wide service — the
// refused exporter must not receive anything logged afterwards.
// Discriminating evidence: a publish-then-forgotten-rollback
// implementation would have it attached to the shared service and
// receiving.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn refused_exporter_registration_leaves_no_trace() {
    let root = Context::new();
    let captured: Captured = Default::default();
    let fiber_handle = root
        .spawn(PreparedPlugin::from_input(
            ContextGrabber {
                captured: captured.clone(),
            },
            (),
        ))
        .await
        .unwrap();
    let dead = captured.lock().clone().expect("apply ran");
    fiber_handle.dispose().await.unwrap();

    let leaked = Arc::new(BufferExporter::new(4, Level::Info).unwrap());
    dead.add_exporter(leaked.clone())
        .expect_err("registration on a disposed fiber must fail");

    root.logger().info("logged after the refusal");
    assert!(
        leaked.snapshot().is_empty(),
        "the refused exporter must not be attached to the runtime-wide service"
    );
}

#[test]
fn level_order_is_semantic_low_to_high_severity() {
    assert!(Level::Debug < Level::Info && Level::Info < Level::Warn && Level::Warn < Level::Error);
}

#[test]
fn log_record_accessors_are_semantic() {
    let ctx = Context::new();
    let buffer = Arc::new(BufferExporter::new(2, Level::Debug).unwrap());
    let _registration = ctx.add_exporter(buffer.clone()).unwrap();
    ctx.logger().with_name("audit").warn("hello");
    let record = buffer.snapshot().pop().unwrap();
    assert!(record.sequence() > 0);
    let _: std::time::SystemTime = record.timestamp();
    assert_eq!(record.channel(), "audit");
    assert_eq!(record.level(), Level::Warn);
    assert_eq!(record.text(), "hello");
}

#[test]
fn reentrant_exporter_does_not_recurse_and_later_exporter_is_attempted() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Reentrant {
        logger: cordis_core::Logger,
        hits: Arc<AtomicUsize>,
    }
    impl Exporter for Reentrant {
        fn export(&self, _r: &LogRecord) {
            self.hits.fetch_add(1, Ordering::Relaxed);
            self.logger.info("nested");
        }
    }
    struct Later(Arc<AtomicUsize>);
    impl Exporter for Later {
        fn export(&self, _r: &LogRecord) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    let ctx = Context::new();
    let a = Arc::new(AtomicUsize::new(0));
    let b = Arc::new(AtomicUsize::new(0));
    let _ra = ctx
        .add_exporter(Arc::new(Reentrant {
            logger: ctx.logger(),
            hits: a.clone(),
        }))
        .unwrap();
    let _rb = ctx.add_exporter(Arc::new(Later(b.clone()))).unwrap();
    ctx.logger().info("outer");
    assert_eq!(a.load(Ordering::Relaxed), 1);
    assert_eq!(b.load(Ordering::Relaxed), 1);
}

#[test]
fn remove_after_snapshot_affects_only_future_records() {
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Blocking {
        entered: Arc<Barrier>,
        release: Arc<Barrier>,
        hits: Arc<AtomicUsize>,
    }
    impl Exporter for Blocking {
        fn export(&self, _record: &LogRecord) {
            self.entered.wait();
            self.release.wait();
            self.hits.fetch_add(1, Ordering::SeqCst);
        }
    }

    let ctx = Context::new();
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let hits = Arc::new(AtomicUsize::new(0));
    let registration = ctx
        .add_exporter(Arc::new(Blocking {
            entered: entered.clone(),
            release: release.clone(),
            hits: hits.clone(),
        }))
        .unwrap();
    let logger = ctx.logger();
    let worker = std::thread::spawn(move || logger.info("snapshotted"));

    entered.wait();
    assert!(registration.remove());
    release.wait();
    worker.join().unwrap();
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "the retained snapshot completes"
    );

    ctx.logger().info("future");
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "removal excludes future snapshots"
    );
}

#[test]
fn panicking_exporter_does_not_block_later_occurrence() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Panics;
    impl Exporter for Panics {
        fn export(&self, _record: &LogRecord) {
            panic!("boom");
        }
    }
    struct Later(Arc<AtomicUsize>);
    impl Exporter for Later {
        fn export(&self, _record: &LogRecord) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let ctx = Context::new();
    let hits = Arc::new(AtomicUsize::new(0));
    let _first = ctx.add_exporter(Arc::new(Panics)).unwrap();
    let _second = ctx.add_exporter(Arc::new(Later(hits.clone()))).unwrap();
    ctx.logger().info("survives");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[test]
fn reentrant_filter_does_not_recurse_through_logging() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct ReentrantFilter {
        logger: cordis_core::Logger,
        filters: Arc<AtomicUsize>,
    }
    impl Exporter for ReentrantFilter {
        fn export(&self, _record: &LogRecord) {}
        fn min_level(&self, _channel: &str) -> Option<Level> {
            self.filters.fetch_add(1, Ordering::SeqCst);
            self.logger.info("nested from filter");
            None
        }
    }
    let ctx = Context::new();
    let filters = Arc::new(AtomicUsize::new(0));
    let _registration = ctx
        .add_exporter(Arc::new(ReentrantFilter {
            logger: ctx.logger(),
            filters: filters.clone(),
        }))
        .unwrap();
    ctx.logger().info("outer");
    assert_eq!(filters.load(Ordering::SeqCst), 1);
}

#[test]
fn logging_does_not_create_service_visibility_or_missing_edges() {
    let ctx = Context::new();
    let before = ctx.runtime_snapshot();
    let before_services = before.services().len();
    let before_missing = before.fibers()[0].missing_services().to_vec();
    ctx.logger().debug("foundation logger");
    ctx.logger().with_name("derived").error("still foundation");
    let after = ctx.runtime_snapshot();
    assert_eq!(after.services().len(), before_services);
    assert_eq!(after.fibers()[0].missing_services(), before_missing);
}

#[test]
fn buffer_retains_exporter_receipt_order_not_sequence_order() {
    use std::sync::Barrier;

    struct Gate {
        entered: Arc<Barrier>,
        release: Arc<Barrier>,
    }
    impl Exporter for Gate {
        fn export(&self, record: &LogRecord) {
            if record.text() == "first" {
                self.entered.wait();
                self.release.wait();
            }
        }
        fn default_level(&self) -> Level {
            Level::Debug
        }
    }

    let ctx = Context::new();
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let _gate = ctx
        .add_exporter(Arc::new(Gate {
            entered: entered.clone(),
            release: release.clone(),
        }))
        .unwrap();
    let buffer = Arc::new(BufferExporter::new(4, Level::Debug).unwrap());
    let _buffer = ctx.add_exporter(buffer.clone()).unwrap();

    let logger = ctx.logger();
    let first = logger.clone();
    let worker = std::thread::spawn(move || first.info("first"));
    entered.wait();
    logger.info("second");
    release.wait();
    worker.join().unwrap();

    let snapshot = buffer.snapshot();
    assert_eq!(
        snapshot.iter().map(LogRecord::text).collect::<Vec<_>>(),
        ["second", "first"]
    );
    assert!(
        snapshot[0].sequence() > snapshot[1].sequence(),
        "receipt order is intentionally independent of assignment sequence"
    );
}

#[tokio::test]
async fn logger_remains_foundation_available_across_generation_phases() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct PhasePlugin {
        attempts: Arc<AtomicUsize>,
        captured: Captured,
    }
    impl Plugin for PhasePlugin {
        type Config = ();
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = std::io::Error;

        fn name(&self) -> Cow<'static, str> {
            Cow::Borrowed("PhaseProbe")
        }
        fn prepare(&self, (): ()) -> Result<(), Infallible> {
            Ok(())
        }
        async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), std::io::Error> {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
            ctx.logger().info(format!("apply-{attempt}"));
            *self.captured.lock() = Some(ctx.clone());
            if attempt == 1 {
                let cleanup_logger = ctx.logger();
                ctx.effect_sync(move || cleanup_logger.warn("generation-closing"))
                    .map_err(|error| std::io::Error::other(error.to_string()))?;
                Ok(())
            } else {
                Err(std::io::Error::other("planned restart failure"))
            }
        }
    }

    let root = Context::new();
    let buffer = Arc::new(BufferExporter::new(16, Level::Debug).unwrap());
    let _buffer_registration = root.add_exporter(buffer.clone()).unwrap();
    let captured: Captured = Default::default();
    let fiber_handle = root
        .spawn(PreparedPlugin::from_input(
            PhasePlugin {
                attempts: Arc::new(AtomicUsize::new(0)),
                captured: captured.clone(),
            },
            (),
        ))
        .await
        .unwrap();
    fiber_handle.ready().await.unwrap();

    fiber_handle
        .restart()
        .await
        .expect_err("second apply intentionally fails");
    assert_eq!(fiber_handle.state(), cordis_core::FiberState::Failed);
    captured
        .lock()
        .clone()
        .expect("failed apply kept its Context")
        .logger()
        .error("failed-state");

    let texts = buffer
        .snapshot()
        .into_iter()
        .map(|record| record.text().to_owned())
        .collect::<Vec<_>>();
    assert!(
        texts.iter().any(|text| text == "apply-1"),
        "Logger works during initial settle/apply"
    );
    assert!(
        texts.iter().any(|text| text == "generation-closing"),
        "Logger works while generation cleanup closes"
    );
    assert!(
        texts.iter().any(|text| text == "apply-2"),
        "Logger works during restart settle/apply"
    );
    assert!(
        texts.iter().any(|text| text == "failed-state"),
        "Logger remains available on a Failed Fiber"
    );
}

#[test]
fn exporter_reentrancy_guard_is_runtime_local() {
    struct Bridge(cordis_core::Logger);
    impl Exporter for Bridge {
        fn export(&self, _record: &LogRecord) {
            self.0.info("bridged-to-other-runtime");
        }
    }
    let source = Context::new();
    let destination = Context::new();
    let buffer = Arc::new(BufferExporter::new(4, Level::Debug).unwrap());
    let _destination = destination.add_exporter(buffer.clone()).unwrap();
    let _bridge = source
        .add_exporter(Arc::new(Bridge(destination.logger())))
        .unwrap();
    source.logger().info("source");
    assert_eq!(buffer.snapshot().len(), 1);
    assert_eq!(buffer.snapshot()[0].text(), "bridged-to-other-runtime");
}

#[tokio::test]
async fn exact_remove_can_win_while_generation_drain_is_in_progress() {
    use cordis_core::logger::ExporterRegistration;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    struct Counter(Arc<AtomicUsize>);
    impl Exporter for Counter {
        fn export(&self, _record: &LogRecord) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct DrainRacePlugin {
        counter: Arc<AtomicUsize>,
        registration: Arc<Mutex<Option<ExporterRegistration>>>,
        cleanup_started: Arc<Notify>,
        cleanup_release: Arc<Notify>,
    }
    impl Plugin for DrainRacePlugin {
        type Config = ();
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = std::io::Error;

        fn prepare(&self, (): ()) -> Result<(), Infallible> {
            Ok(())
        }
        async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), std::io::Error> {
            let registration = ctx
                .add_exporter(Arc::new(Counter(self.counter.clone())))
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            *self.registration.lock() = Some(registration);
            let started = self.cleanup_started.clone();
            let release = self.cleanup_release.clone();
            ctx.effect(move || async move {
                started.notify_one();
                release.notified().await;
            })
            .map_err(|error| std::io::Error::other(error.to_string()))?;
            Ok(())
        }
    }

    let root = Context::new();
    let counter = Arc::new(AtomicUsize::new(0));
    let registration = Arc::new(Mutex::new(None));
    let cleanup_started = Arc::new(Notify::new());
    let cleanup_release = Arc::new(Notify::new());
    let fiber_handle = root
        .spawn(PreparedPlugin::from_input(
            DrainRacePlugin {
                counter: counter.clone(),
                registration: registration.clone(),
                cleanup_started: cleanup_started.clone(),
                cleanup_release: cleanup_release.clone(),
            },
            (),
        ))
        .await
        .unwrap();
    fiber_handle.ready().await.unwrap();
    let exact = registration
        .lock()
        .take()
        .expect("apply published exporter");
    root.logger().info("before drain");
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    let dispose = tokio::spawn(async move { fiber_handle.dispose().await });
    cleanup_started.notified().await;
    assert!(exact.remove());
    root.logger().info("removed during drain");
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    cleanup_release.notify_one();
    dispose.await.unwrap().unwrap();
}
