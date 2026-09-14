//! EX-04 executable evidence for the final-v3 worker daemon consumer.

use parking_lot::Mutex;
use std::convert::Infallible;
use std::fs;
use std::process::{Command, Stdio};
use std::time::Duration;

use cordis_core::{Context, Plugin, PreparedPlugin};
use cordis_timer::{TimerExt, TimerRegistrationError};
use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};

#[test]
fn worker_daemon_uses_only_final_v3_facades() {
    let source = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"))
        .expect("worker_daemon source should be readable");
    for stale in [
        "internal::",
        "with_state",
        ".plugin(",
        "tokio::spawn",
        "live_fiber_count",
        "registry",
        "CordisError",
        "EventCarrier",
    ] {
        assert!(!source.contains(stale), "stale mechanism `{stale}` remains");
    }
    for required in [
        "PreparedPlugin",
        "PreparedChange",
        "InjectSpec::none().require",
        ".run(",
        ".sleep(",
        ".interval(",
        "runtime_snapshot()",
        ".restart().await",
        ".update(",
        "Roster",
    ] {
        assert!(
            source.contains(required),
            "final-v3 mechanism `{required}` is not demonstrated"
        );
    }
}

#[test]
fn worker_daemon_runbook_is_scripted_self_terminating_and_discriminating() {
    let output = Command::new(env!("CARGO_BIN_EXE_worker_daemon"))
        .stdin(Stdio::null())
        .output()
        .expect("worker_daemon should launch");
    assert!(
        output.status.success(),
        "worker_daemon exited unsuccessfully:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8(output.stdout).expect("worker_daemon output is UTF-8");
    for evidence in [
        "service convergence: worker pending",
        "service convergence: worker active",
        "sleep backoff: completed",
        "exact update: committed",
        "exact restart: generation advanced",
        "flat snapshot:",
        "teardown cancellation: interval cancelled",
        "reverse roster disposal:",
        "worker daemon down",
    ] {
        assert!(
            stdout.contains(evidence),
            "missing `{evidence}` in:\n{stdout}"
        );
    }
}

struct TimerProbe {
    context: Mutex<Option<oneshot::Sender<Context>>>,
    events: mpsc::UnboundedSender<&'static str>,
}

impl Plugin for TimerProbe {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _input: &()) -> Result<(), Infallible> {
        if let Some(sender) = self.context.lock().take() {
            let _ = sender.send(ctx.clone());
        }
        let task_ctx = ctx.clone();
        let events = self.events.clone();
        ctx.run(async move {
            let mut ticks = task_ctx
                .interval(Duration::from_millis(100))
                .expect("interval registers");
            let _ = events.send("armed");
            loop {
                match ticks.next().await {
                    Some(Ok(())) => {
                        let _ = events.send("tick");
                    }
                    Some(Err(_)) => {
                        let _ = events.send("cancelled");
                        break;
                    }
                    None => break,
                }
            }
        })
        .expect("run registers");
        Ok(())
    }
}

#[tokio::test(start_paused = true)]
async fn interval_keeps_construction_phase_and_generation_teardown_cancels_it() {
    let root = Context::new();
    let (context_tx, context_rx) = oneshot::channel();
    let (events_tx, mut events_rx) = mpsc::unbounded_channel();
    let plugin = TimerProbe {
        context: Mutex::new(Some(context_tx)),
        events: events_tx,
    };
    plugin.prepare(()).unwrap();
    let fiber_handle = root
        .spawn(PreparedPlugin::from_input(plugin, ()))
        .await
        .unwrap();
    let generation_ctx = context_rx.await.unwrap();
    assert_eq!(events_rx.recv().await, Some("armed"));

    tokio::time::advance(Duration::from_millis(100)).await;
    assert_eq!(events_rx.recv().await, Some("tick"));

    tokio::time::advance(Duration::from_millis(250)).await;
    assert_eq!(events_rx.recv().await, Some("tick"));
    tokio::task::yield_now().await;
    assert!(events_rx.try_recv().is_err(), "missed ticks coalesce");

    tokio::time::advance(Duration::from_millis(49)).await;
    tokio::task::yield_now().await;
    assert!(
        events_rx.try_recv().is_err(),
        "phase has not reached 400 ms"
    );
    tokio::time::advance(Duration::from_millis(1)).await;
    assert_eq!(events_rx.recv().await, Some("tick"));

    fiber_handle.dispose().await.unwrap();
    assert_eq!(events_rx.recv().await, Some("cancelled"));
    assert!(matches!(
        generation_ctx.sleep(Duration::from_millis(1)),
        Err(TimerRegistrationError::InactiveContext)
    ));
}
