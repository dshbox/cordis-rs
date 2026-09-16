//! logging_exporters — final-v3 Logger, Runtime observation, and typed-removal tour.
//!
//! The runbook demonstrates named Logger channels, per-exporter filtering, explicit bounded
//! buffering, exact exporter occurrence removal, immutable LogRecord accessors, flat Runtime
//! snapshots, detached best-effort Runtime observation, and typed Plugin removal administration.
//! No TTY input is required.

use std::borrow::Cow;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cordis_core::event::{observer, observer_sync};
use cordis_core::logger::{BufferExporter, Exporter, ExporterRegistration, LogRecord};
use cordis_core::observation::{ResidencyChange, RuntimeObservation};
use cordis_core::{BoxError, Context, FiberId, Level, Plugin, PreparedPlugin};
use examples_common::{Roster, section};
use parking_lot::Mutex;

struct RecordingExporter {
    records: Mutex<Vec<LogRecord>>,
    per_channel: HashMap<&'static str, Level>,
    default: Level,
}

impl RecordingExporter {
    fn new(per_channel: HashMap<&'static str, Level>, default: Level) -> Self {
        Self {
            records: Mutex::new(Vec::new()),
            per_channel,
            default,
        }
    }
    fn snapshot(&self) -> Vec<LogRecord> {
        self.records.lock().clone()
    }
}

impl Exporter for RecordingExporter {
    fn export(&self, record: &LogRecord) {
        self.records.lock().push(record.clone());
    }
    fn min_level(&self, channel: &str) -> Option<Level> {
        self.per_channel.get(channel).copied()
    }
    fn default_level(&self) -> Level {
        self.default
    }
}

struct Canary;
impl Plugin for Canary {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn name(&self) -> Cow<'_, str> {
        "logging-canary".into()
    }
    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, _ctx: Context, (): &()) -> Result<(), Infallible> {
        Ok(())
    }
}

fn prepared_canary() -> PreparedPlugin {
    PreparedPlugin::from_input(Canary, ())
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let ctx = Context::new();

    section("Logger channels, filtering, buffering, exact occurrence control");
    ctx.logger()
        .with_name("audit")
        .warn("before buffer registration");

    let buffer = Arc::new(BufferExporter::new(2, Level::Debug)?);
    let _buffer_registration: ExporterRegistration = ctx.add_exporter(buffer.clone())?;
    assert!(buffer.snapshot().is_empty());
    println!("  buffer: pre-registration record absent");
    ctx.logger().debug("buffer-one");
    ctx.logger().debug("buffer-two");
    ctx.logger().debug("buffer-three");
    let buffered = buffer.snapshot();
    assert_eq!(buffered.len(), 2);
    assert_eq!(buffered[0].text(), "buffer-two");
    assert_eq!(buffered[1].text(), "buffer-three");
    println!("  buffering: explicit bounded buffer evicted oldest receipt");

    let filtered = Arc::new(RecordingExporter::new(
        HashMap::from([("chatty", Level::Warn)]),
        Level::Info,
    ));
    let _filtered_registration = ctx.add_exporter(filtered.clone())?;
    ctx.logger()
        .with_name("chatty")
        .info("filtered chatty info");
    ctx.logger()
        .with_name("chatty")
        .warn("accepted chatty warn");
    ctx.logger().with_name("audit").info("accepted audit info");
    let filtered_records = filtered.snapshot();
    assert_eq!(filtered_records.len(), 2);
    assert!(
        filtered_records
            .iter()
            .any(|r| r.channel() == "chatty" && r.level() == Level::Warn)
    );
    assert!(
        filtered_records
            .iter()
            .any(|r| r.channel() == "audit" && r.level() == Level::Info)
    );
    println!("  filtering: channel override and default threshold distinguished");

    let exact = Arc::new(RecordingExporter::new(HashMap::new(), Level::Debug));
    let removed_occurrence = ctx.add_exporter(exact.clone())?;
    let _surviving_occurrence = ctx.add_exporter(exact.clone())?;
    ctx.logger().debug("exact-before-remove");
    assert!(removed_occurrence.remove());
    ctx.logger().debug("exact-after-remove");
    let exact_records = exact.snapshot();
    assert_eq!(
        exact_records
            .iter()
            .filter(|r| r.text() == "exact-before-remove")
            .count(),
        2,
    );
    assert_eq!(
        exact_records
            .iter()
            .filter(|r| r.text() == "exact-after-remove")
            .count(),
        1,
    );
    println!("  exact removal: removed occurrence stayed quiet while sibling survived");

    let record = buffer
        .snapshot()
        .last()
        .expect("buffer received post-registration logs")
        .clone();
    let _ = record.sequence();
    let _ = record.timestamp();
    assert!(!record.channel().is_empty());
    let _ = record.level().as_str();
    assert!(!record.text().is_empty());
    println!("  record accessors: sequence/timestamp/channel/level/text observed");

    section("flat Runtime snapshot and best-effort observation");
    let initial = ctx.runtime_snapshot();
    assert_eq!(
        initial.fibers().len(),
        1,
        "root is the only initial Fiber row"
    );
    println!("  snapshot: flat fiber rows observed");

    ctx.observe_runtime(observer(|_: Context, _: RuntimeObservation| async {
        Err::<(), _>(std::io::Error::other("deliberate observer failure"))
    }))?;
    let observation_hits = Arc::new(AtomicUsize::new(0));
    let observed_fiber = Arc::new(Mutex::new(None::<FiberId>));
    let (observed_tx, observed_rx) = tokio::sync::oneshot::channel::<()>();
    let observed_tx = Arc::new(Mutex::new(Some(observed_tx)));
    let hits = observation_hits.clone();
    let observed_id = observed_fiber.clone();
    let signal = observed_tx.clone();
    ctx.observe_runtime(observer_sync(
        move |_: Context, record: RuntimeObservation| {
            if let RuntimeObservation::FiberResidency {
                change: ResidencyChange::Admitted,
                fiber,
            } = record
            {
                hits.fetch_add(1, Ordering::SeqCst);
                let mut slot = observed_id.lock();
                if slot.is_none() {
                    *slot = Some(fiber.id().clone());
                    if let Some(tx) = signal.lock().take() {
                        let _ = tx.send(());
                    }
                }
            }
            Ok::<(), Infallible>(())
        },
    ))?;

    let mut roster = Roster::new();
    let first = roster.push(ctx.spawn(prepared_canary()).await?);
    let second = roster.push(ctx.spawn(prepared_canary()).await?);
    roster.report().await;
    let mut observed_rx = observed_rx;
    let mut observed = false;
    for _ in 0..100_000 {
        match observed_rx.try_recv() {
            Ok(()) => {
                observed = true;
                break;
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => tokio::task::yield_now().await,
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => break,
        }
    }
    assert!(
        observed,
        "successful observer receives detached residency observation"
    );
    assert!(observation_hits.load(Ordering::SeqCst) > 0);
    let observed_id = observed_fiber
        .lock()
        .clone()
        .expect("admission id was observed");
    assert!(
        ctx.runtime_snapshot()
            .fibers()
            .iter()
            .any(|row| row.id() == &observed_id),
        "an observed admitted Fiber is recoverable in the flat current snapshot",
    );
    println!("  snapshot/observation: opaque FiberId correlates current state");
    assert_eq!(first.state(), cordis_core::FiberState::Active);
    assert_eq!(second.state(), cordis_core::FiberState::Active);
    println!("  observation: failing observer did not fail source operation");

    section("typed group removal and fresh later spawn");
    let old_ids = [first.id().clone(), second.id().clone()];
    assert_eq!(
        ctx.runtime_snapshot()
            .fibers()
            .iter()
            .filter(|row| row.name() == "logging-canary")
            .count(),
        2,
    );
    ctx.remove_plugins::<Canary>().await?;
    assert_eq!(
        ctx.runtime_snapshot()
            .fibers()
            .iter()
            .filter(|row| row.name() == "logging-canary")
            .count(),
        0,
    );
    println!("  typed removal: old rows disappeared");

    let fresh = roster.push(ctx.spawn(prepared_canary()).await?);
    assert!(old_ids.iter().all(|old| old.clone() != fresh.id()));
    assert_eq!(
        ctx.runtime_snapshot()
            .fibers()
            .iter()
            .filter(|row| row.name() == "logging-canary")
            .count(),
        1,
    );
    println!("  fresh spawn: same Plugin received a fresh FiberId");

    section("teardown");
    roster.teardown().await;
    assert_eq!(
        ctx.runtime_snapshot()
            .fibers()
            .iter()
            .filter(|row| row.name() == "logging-canary")
            .count(),
        0,
    );
    println!("  logging_exporters tour complete");
    Ok(())
}
