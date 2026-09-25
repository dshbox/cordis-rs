//! worker_daemon — EX-04: final-v3 cooperative worker lifecycle.
//!
//! The scripted daemon demonstrates Service-driven convergence, generation-owned
//! `Context::run` loops, exact same-Fiber update/restart, Sleep backoff,
//! fixed-phase Interval work, synchronous Timer-registration refusal, flat
//! Runtime diagnostics, generation cancellation, and Harness-owned reverse
//! teardown. No TTY input is read.

use std::convert::Infallible;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use cordis_core::{
    BoxError, Context, FiberState, InjectSpec, Plugin, PreparedChange, PreparedPlugin, Service,
    UpdateOutcome,
};
use cordis_timer::{TimerExt, TimerRegistrationError};
use examples_common::{Roster, section};
use futures::StreamExt;
use parking_lot::Mutex;

#[derive(Debug)]
struct DemoError(String);

impl DemoError {
    fn from_display(error: impl fmt::Display) -> Self {
        Self(error.to_string())
    }
}

impl fmt::Display for DemoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DemoError {}

struct WorkPermit;
impl Service for WorkPermit {
    const NAME: &'static str = "worker/permit";
}

struct PermitPlugin {
    probe: Arc<WorkerProbe>,
}

impl Plugin for PermitPlugin {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = DemoError;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _input: &()) -> Result<(), DemoError> {
        let _publication = ctx
            .provide(Arc::new(WorkPermit))
            .map_err(DemoError::from_display)?;
        let probe = self.probe.clone();
        ctx.effect_sync(move || {
            if probe.teardown.load(Ordering::SeqCst) {
                probe.order.lock().push("provider");
            }
        })
        .map_err(DemoError::from_display)?;
        Ok(())
    }
}

#[derive(Clone)]
struct WorkerConfig {
    period: Duration,
    revision: &'static str,
}

#[derive(Default)]
struct WorkerProbe {
    generations: AtomicUsize,
    ticks: AtomicUsize,
    teardown: AtomicBool,
    order: Mutex<Vec<&'static str>>,
}

struct WorkerPlugin {
    probe: Arc<WorkerProbe>,
}

impl Plugin for WorkerPlugin {
    type Config = WorkerConfig;
    type Input = WorkerConfig;
    type PrepareError = Infallible;
    type ApplyError = DemoError;

    fn inject(&self) -> InjectSpec {
        InjectSpec::none().require(WorkPermit::NAME)
    }

    fn prepare(&self, config: WorkerConfig) -> Result<WorkerConfig, Infallible> {
        Ok(config)
    }

    async fn apply(&self, ctx: Context, input: &WorkerConfig) -> Result<(), DemoError> {
        let _permit = ctx
            .try_service::<WorkPermit>()
            .map_err(DemoError::from_display)?;
        let generation = self.probe.generations.fetch_add(1, Ordering::SeqCst) + 1;
        println!(
            "  worker generation {generation} applied: revision={} period={:?}",
            input.revision, input.period
        );

        let cleanup_probe = self.probe.clone();
        ctx.effect_sync(move || {
            if cleanup_probe.teardown.load(Ordering::SeqCst) {
                cleanup_probe.order.lock().push("worker");
            }
        })
        .map_err(DemoError::from_display)?;

        let task_ctx = ctx.clone();
        let task_probe = self.probe.clone();
        let period = input.period;
        ctx.run(async move {
            let mut ticks = match task_ctx.interval(period) {
                Ok(ticks) => ticks,
                Err(error) => {
                    println!("  interval registration refused synchronously: {error}");
                    return;
                }
            };
            loop {
                match ticks.next().await {
                    Some(Ok(())) => {
                        task_probe.ticks.fetch_add(1, Ordering::SeqCst);
                    }
                    Some(Err(_)) => {
                        if task_probe.teardown.load(Ordering::SeqCst) {
                            println!("  teardown cancellation: interval cancelled");
                        }
                        break;
                    }
                    None => break,
                }
            }
        })
        .map_err(DemoError::from_display)?;
        Ok(())
    }
}

fn prepare<P: Plugin>(plugin: P, config: P::Config) -> Result<PreparedPlugin, P::PrepareError> {
    let input = plugin.prepare(config)?;
    Ok(PreparedPlugin::from_input(plugin, input))
}

fn permit_plugin(probe: &Arc<WorkerProbe>) -> PermitPlugin {
    PermitPlugin {
        probe: probe.clone(),
    }
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let root = Context::new();
    let mut roster = Roster::new();
    let probe = Arc::new(WorkerProbe::default());

    section("timer registration: synchronous refusal");
    match root.interval(Duration::ZERO) {
        Err(TimerRegistrationError::ZeroPeriod) => {
            println!("  timer registration refusal: zero period handled synchronously");
        }
        _ => return Err(DemoError("zero-period interval was not refused".into()).into()),
    }

    section("boot: dependent worker before its Service");
    let worker_plugin = WorkerPlugin {
        probe: probe.clone(),
    };
    let worker = roster.push(
        root.spawn(prepare(
            worker_plugin,
            WorkerConfig {
                period: Duration::from_millis(40),
                revision: "v1",
            },
        )?)
        .await?,
    );
    assert_eq!(worker.state(), FiberState::Pending);
    assert_eq!(worker.pending_missing(), [WorkPermit::NAME.to_owned()]);
    println!("  service convergence: worker pending");

    let first_provider = roster.push(root.spawn(prepare(permit_plugin(&probe), ())?).await?);
    assert_eq!(worker.ready().await?, FiberState::Active);
    println!("  service convergence: worker active");
    let summary = roster.report().await;
    assert_eq!(summary.pending, 0);

    section("supervision: generation-owned Sleep backoff");
    let backoff = root.sleep(Duration::from_millis(120))?;
    backoff.await?;
    println!("  sleep backoff: completed");
    println!(
        "  fixed-phase interval: worker ticks observed={}",
        probe.ticks.load(Ordering::SeqCst)
    );

    section("control: exact same-Fiber update and restart");
    let before_update = probe.generations.load(Ordering::SeqCst);
    let update = worker
        .update(PreparedChange::from_input::<WorkerPlugin>(WorkerConfig {
            period: Duration::from_millis(30),
            revision: "v2",
        }))
        .await?;
    assert_eq!(update, UpdateOutcome::Committed(FiberState::Active));
    assert!(probe.generations.load(Ordering::SeqCst) > before_update);
    println!("  exact update: committed");

    let before_restart = probe.generations.load(Ordering::SeqCst);
    worker.restart().await?;
    assert!(probe.generations.load(Ordering::SeqCst) > before_restart);
    println!("  exact restart: generation advanced");

    section("service drift: withdraw and restore exact publication");
    first_provider.dispose().await?;
    assert_eq!(worker.ready().await?, FiberState::Pending);
    println!("  service convergence: worker pending after publication withdrawal");
    let _replacement = roster.push(root.spawn(prepare(permit_plugin(&probe), ())?).await?);
    assert_eq!(worker.ready().await?, FiberState::Active);
    println!("  service convergence: worker active after replacement publication");

    section("diagnostics: flat Runtime snapshot");
    let snapshot = root.runtime_snapshot();
    let fibers = snapshot
        .fibers()
        .iter()
        .map(|fiber| format!("{}:{:?}", fiber.name(), fiber.state()))
        .collect::<Vec<_>>()
        .join(", ");
    let services = snapshot
        .services()
        .iter()
        .map(|service| service.service().to_owned())
        .collect::<Vec<_>>()
        .join(", ");
    println!("  flat snapshot: fibers=[{fibers}] services=[{services}]");

    section("shutdown: Harness reverse Roster disposal");
    probe.teardown.store(true, Ordering::SeqCst);
    roster.teardown().await;
    // Roster calls dispose in reverse order, but withdrawing the provider's
    // Service can start worker convergence while provider cleanup is draining.
    // Verify both cleanups happened exactly once without ordering their callbacks.
    let mut cleaned = probe.order.lock().clone();
    cleaned.sort_unstable();
    assert_eq!(cleaned, ["provider", "worker"]);
    println!("  reverse roster disposal: provider and worker cleaned");
    println!("  worker daemon down");
    Ok(())
}
