//! EX-01 contract evidence for the shared consumer Harness Roster.

use cordis_core::{
    Context, FiberState, InjectSpec, Plugin, PreparedChange, PreparedPlugin, lifecycle::FiberRole,
};
use examples_common::{BootSummary, Roster, boot_report, teardown};
use std::{convert::Infallible, error::Error, fmt, sync::Arc};

#[derive(Debug)]
struct ApplyFailure(&'static str);
impl fmt::Display for ApplyFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl Error for ApplyFailure {}

struct Up;
impl Plugin for Up {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, _ctx: Context, _input: &()) -> Result<(), Infallible> {
        Ok(())
    }
}

struct Waiting;
impl Plugin for Waiting {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn inject(&self) -> InjectSpec {
        InjectSpec::none().require("never-provided")
    }
    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, _ctx: Context, _input: &()) -> Result<(), Infallible> {
        Ok(())
    }
}

struct Failable;
impl Plugin for Failable {
    type Config = bool;
    type Input = bool;
    type PrepareError = Infallible;
    type ApplyError = ApplyFailure;
    fn prepare(&self, fail: bool) -> Result<bool, Infallible> {
        Ok(fail)
    }
    async fn apply(&self, _ctx: Context, fail: &bool) -> Result<(), ApplyFailure> {
        if *fail {
            Err(ApplyFailure("bootstrap broken"))
        } else {
            Ok(())
        }
    }
}

struct Recorder {
    name: &'static str,
    order: Arc<parking_lot::Mutex<Vec<&'static str>>>,
}
impl Plugin for Recorder {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = cordis_core::effect::EffectRegistrationError;
    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, ctx: Context, _input: &()) -> Result<(), Self::ApplyError> {
        let (name, order) = (self.name, self.order.clone());
        ctx.effect_sync(move || order.lock().push(name))?;
        Ok(())
    }
}

fn prepared<P: Plugin<Config = (), Input = (), PrepareError = Infallible>>(
    plugin: P,
) -> PreparedPlugin {
    PreparedPlugin::from_input(plugin, ())
}

#[tokio::test]
async fn boot_report_counts_active_pending_and_delivered_failed_fiber_handles() {
    let ctx = Context::new();
    let up = ctx.spawn(prepared(Up)).await.unwrap();
    let waiting = ctx.spawn(prepared(Waiting)).await.unwrap();
    let failed = ctx
        .spawn(PreparedPlugin::from_input(Failable, false))
        .await
        .unwrap();
    failed
        .update(PreparedChange::from_input::<Failable>(true))
        .await
        .expect_err("committed update apply fails");
    assert_eq!(failed.state(), FiberState::Failed);

    assert_eq!(
        boot_report(&[up, waiting, failed]).await,
        BootSummary {
            up: 1,
            pending: 1,
            failed: 1
        }
    );
}

#[tokio::test]
async fn initial_apply_rejection_never_enters_the_roster() {
    let ctx = Context::new();
    let roster = Roster::new();
    let before = ctx.runtime_snapshot().fibers().len();
    assert!(
        ctx.spawn(PreparedPlugin::from_input(Failable, true))
            .await
            .is_err()
    );
    assert_eq!(roster.report().await, BootSummary::default());
    assert_eq!(
        ctx.runtime_snapshot().fibers().len(),
        before,
        "rejected initial apply leaves no attempted resident"
    );
}

#[tokio::test]
async fn boot_report_summary_is_all_up_when_every_fiber_handle_activates() {
    let ctx = Context::new();
    let fiber_handle = ctx.spawn(prepared(Up)).await.unwrap();
    assert_eq!(
        boot_report(&[fiber_handle]).await,
        BootSummary {
            up: 1,
            pending: 0,
            failed: 0
        }
    );
}

#[tokio::test]
async fn teardown_disposes_in_reverse_spawn_order_attempt_all() {
    let ctx = Context::new();
    let order = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let mut spawned = Vec::new();
    for name in ["alpha", "beta", "gamma"] {
        spawned.push(
            ctx.spawn(prepared(Recorder {
                name,
                order: order.clone(),
            }))
            .await
            .unwrap(),
        );
    }
    teardown(&spawned).await;
    assert_eq!(*order.lock(), ["gamma", "beta", "alpha"]);
    assert!(
        spawned
            .iter()
            .all(|fiber_handle| fiber_handle.state() == FiberState::Disposed)
    );
    assert_eq!(
        ctx.runtime_snapshot()
            .fibers()
            .iter()
            .filter(|f| f.role() == FiberRole::Ordinary)
            .count(),
        0
    );
}

#[tokio::test]
async fn teardown_is_idempotent_across_repeat_calls() {
    let ctx = Context::new();
    let fiber_handle = ctx.spawn(prepared(Up)).await.unwrap();
    teardown(std::slice::from_ref(&fiber_handle)).await;
    teardown(&[fiber_handle]).await;
    assert_eq!(
        ctx.runtime_snapshot()
            .fibers()
            .iter()
            .filter(|f| f.role() == FiberRole::Ordinary)
            .count(),
        0
    );
}

#[tokio::test]
async fn roster_push_returns_the_same_handle_it_records() {
    let ctx = Context::new();
    let mut roster = Roster::new();
    let spawned = ctx.spawn(prepared(Up)).await.unwrap();
    let id = spawned.id().clone();
    let returned = roster.push(spawned);
    assert_eq!(
        returned.id(),
        id,
        "push returns the exact delivered FiberHandle"
    );
    returned.dispose().await.unwrap();
    assert_eq!(roster.report().await, BootSummary::default());
}

#[tokio::test]
async fn roster_holds_spawn_order_across_a_mid_flow_report() {
    let ctx = Context::new();
    let order = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let mut roster = Roster::new();
    roster.push(
        ctx.spawn(prepared(Recorder {
            name: "alpha",
            order: order.clone(),
        }))
        .await
        .unwrap(),
    );
    let mid = roster.report().await;
    roster.push(
        ctx.spawn(prepared(Recorder {
            name: "beta",
            order: order.clone(),
        }))
        .await
        .unwrap(),
    );
    assert_eq!(
        mid,
        BootSummary {
            up: 1,
            pending: 0,
            failed: 0
        }
    );
    roster.teardown().await;
    assert_eq!(*order.lock(), ["beta", "alpha"]);
}
