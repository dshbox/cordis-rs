//! `FiberHandle::wait_state` deadlines share one scheduler thread.
//!
//! Each pending wait used to park its own OS thread until timeout or
//! cancellation, so concurrent waits scaled linearly in threads and a refused
//! thread spawn panicked inside `wait_state`. Pending waits now cost one
//! scheduler entry each, served by a single lazily started thread.
//!
//! This binary holds exactly one test so no concurrently running test can
//! perturb the process thread count it measures (Linux `/proc` accounting).

use std::convert::Infallible;
use std::time::Duration;

use cordis_core::{Context, FiberState, Plugin, PreparedPlugin};

struct Idle;

impl Plugin for Idle {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, _ctx: Context, _: &()) -> Result<(), Infallible> {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn process_threads() -> usize {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("Threads:"))
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

#[cfg(target_os = "linux")]
fn deadline_threads() -> usize {
    std::fs::read_dir("/proc/self/task")
        .unwrap()
        .filter_map(|task| std::fs::read_to_string(task.ok()?.path().join("comm")).ok())
        .filter(|name| name.trim() == "cordis-deadline")
        .count()
}

#[cfg(target_os = "linux")]
#[test]
fn many_pending_waits_share_one_deadline_thread() {
    const WAITS: usize = 500;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        let ctx = Context::new();
        let fiber = ctx
            .spawn(PreparedPlugin::from_input(Idle, ()))
            .await
            .unwrap();
        let before = process_threads();

        let waits = (0..WAITS)
            .map(|_| {
                let fiber = fiber.clone();
                tokio::spawn(async move {
                    fiber
                        .wait_state(FiberState::Pending, Duration::from_secs(600))
                        .await
                })
            })
            .collect::<Vec<_>>();
        // Current-thread runtime: yielding polls every wait until it parks.
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        assert!(waits.iter().all(|wait| !wait.is_finished()));
        let during = process_threads();
        assert!(
            during <= before + 1,
            "{WAITS} pending waits must share one deadline thread \
             (threads before={before}, during={during})"
        );
        assert_eq!(deadline_threads(), 1);

        // Cancellation drops every arm; the shared thread stays and idles.
        for wait in &waits {
            wait.abort();
        }
        for wait in waits {
            assert!(wait.await.unwrap_err().is_cancelled());
        }
        assert_eq!(deadline_threads(), 1);

        // A wait that does not need a deadline, or one far beyond the
        // monotonic clock, still resolves on publication.
        let unbounded = {
            let fiber = fiber.clone();
            tokio::spawn(async move { fiber.wait_state(FiberState::Disposed, Duration::MAX).await })
        };
        tokio::task::yield_now().await;
        fiber.dispose().await.unwrap();
        unbounded.await.unwrap().unwrap();

        // A short deadline still elapses through the shared thread.
        let other = ctx
            .spawn(PreparedPlugin::from_input(Idle, ()))
            .await
            .unwrap();
        let started = std::time::Instant::now();
        let elapsed = other
            .wait_state(FiberState::Pending, Duration::from_millis(50))
            .await;
        assert!(matches!(
            elapsed,
            Err(cordis_core::lifecycle::WaitStateError::Elapsed)
        ));
        assert!(started.elapsed() >= Duration::from_millis(50));
        other.dispose().await.unwrap();
    });
}
