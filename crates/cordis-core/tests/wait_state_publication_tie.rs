//! `wait_state` observes a publication that precedes its deadline.
//!
//! The requested state is published well before the deadline, but the waiting
//! task is not polled again until the deadline has also fired, as on a busy or
//! blocked executor. On resume the publication wake and the deadline are both
//! ready; publication wins that tie. It used to be a per-poll coin flip, so
//! about half of such waits reported `Elapsed`.
//!
//! One round parks `WAITS` independent waits across a single blocked-executor
//! window: the unfixed coin flip escapes with probability `2^-WAITS` while the
//! test sleeps through only one deadline.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cordis_core::lifecycle::WaitStateError;
use cordis_core::{Context, FiberState, InjectSpec, Plugin, PreparedPlugin, Service};

/// First attempt's deadline; each retry doubles it (200 ms up to 3.2 s).
const TIMEOUT: Duration = Duration::from_millis(200);
const WAITS: usize = 32;
/// Rounds whose publication missed the deadline are retried with a doubled
/// deadline, not counted, so only a slow runner pays for a longer round.
const ATTEMPTS: u32 = 5;
/// Upper bound on waiting for the scheduler thread to fire the deadlines.
const FIRE_BOUND: Duration = Duration::from_secs(10);

struct Db;

impl Service for Db {
    const NAME: &'static str = "tie-db";
}

struct NeedsDb;

impl Plugin for NeedsDb {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn inject(&self) -> InjectSpec {
        InjectSpec::none().require(Db::NAME)
    }

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, _ctx: Context, _: &()) -> Result<(), Infallible> {
        Ok(())
    }
}

/// One tie round: `None` when the precondition (publication strictly before
/// every deadline) did not hold, else the number of waits reporting `Elapsed`.
async fn tie_round(timeout: Duration) -> Option<usize> {
    let ctx = Context::new();
    let handle = ctx
        .spawn(PreparedPlugin::from_input(NeedsDb, ()))
        .await
        .unwrap();
    assert_eq!(handle.state(), FiberState::Pending);

    // Every deadline below is armed after `armed_at`, so none fires before
    // `armed_at + timeout`.
    let armed_at = Instant::now();
    let mut ties = (0..WAITS)
        .map(|_| Box::pin(handle.wait_state(FiberState::Active, timeout)))
        .collect::<Vec<_>>();
    for wait in &mut ties {
        assert!(futures::poll!(wait.as_mut()).is_pending(), "deadline armed");
    }
    // Armed last, for a state never published: the scheduler fires in
    // deadline order, so once this reports `Elapsed` every tie deadline has
    // fired too. It proves the tie really happens instead of assuming it.
    let mut witness = Box::pin(handle.wait_state(FiberState::Failed, timeout));
    assert!(futures::poll!(witness.as_mut()).is_pending());

    let publication = ctx.provide(Arc::new(Db)).unwrap();
    assert_eq!(handle.ready().await.unwrap(), FiberState::Active);
    let published_first = armed_at.elapsed() < timeout;

    // Block the executor: no tie wait is polled until every deadline fired.
    std::thread::sleep(timeout.saturating_sub(armed_at.elapsed()));
    let witnessed = loop {
        if let std::task::Poll::Ready(outcome) = futures::poll!(witness.as_mut()) {
            break outcome;
        }
        assert!(
            armed_at.elapsed() < FIRE_BOUND,
            "the deadline scheduler never fired the witness deadline"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(matches!(witnessed, Err(WaitStateError::Elapsed)));

    let mut elapsed = 0;
    for wait in ties {
        match wait.await {
            Ok(()) => {}
            Err(WaitStateError::Elapsed) => elapsed += 1,
            Err(other) => panic!("unexpected {other:?}"),
        }
    }
    drop(publication);
    handle.dispose().await.unwrap();
    published_first.then_some(elapsed)
}

#[tokio::test(flavor = "current_thread")]
async fn a_publication_preceding_the_deadline_wins_a_late_poll() {
    for attempt in 0..ATTEMPTS {
        if let Some(elapsed) = tie_round(TIMEOUT * 2u32.pow(attempt)).await {
            assert_eq!(
                elapsed, 0,
                "{elapsed}/{WAITS} waits reported Elapsed although Active was published first"
            );
            return;
        }
    }
    let last = TIMEOUT * 2u32.pow(ATTEMPTS - 1);
    panic!("precondition never held: publication missed even a {last:?} deadline");
}
