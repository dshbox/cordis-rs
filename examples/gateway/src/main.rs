//! gateway — EX-03: final-v3 declarative boot and live gateway runbook.
//!
//! The consumer freezes an immutable Loader plan, resolves and prepares typed
//! Plugins outside Loader, retains every delivered FiberHandle in the shared Harness,
//! dispatches Events with explicit scoped routing, applies typed same-Fiber
//! update control, uses exact Service publication capabilities, and exercises
//! the complete work-owning Timeout outcome family. No TTY is required.

use std::convert::Infallible;
use std::fmt;
use std::future;
use std::sync::Arc;
use std::time::Duration;

use cordis_core::event::{
    InvocationFailure, ListenerOptions, Next, QueryOutcome, around, mapper_sync, responder_sync,
};
use cordis_core::lifecycle::UpdateNext;
use cordis_core::lifecycle::{UpdateError, UpdateOutcome};
use cordis_core::{
    BoxError, Context, Event, FiberHandle, Plugin, PreparedChange, Routing, Service,
};
use cordis_loader::outcome::EntryOutcome;
use cordis_loader::plan::PluginEntry;
use cordis_loader::resolver::{PluginRequest, prepare_plugin_json};
use cordis_loader::{EntryId, LoadOutcome, LoadPlanBuilder};
use cordis_timer::{TimeoutOutcome, TimerExt};
use examples_common::{Roster, section};
use serde::Deserialize;

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

#[derive(Clone)]
struct Request {
    path: String,
    token: Option<String>,
}

#[derive(Clone)]
struct Response {
    status: u16,
    body: &'static str,
}

impl Response {
    const fn new(status: u16, body: &'static str) -> Self {
        Self { status, body }
    }
}

struct Gate;
impl Event for Gate {
    const NAME: &'static str = "gateway/request";
    type Args = Request;
    type Output = Response;
}

struct Route;
impl Event for Route {
    const NAME: &'static str = "gateway/route";
    type Args = Request;
    type Output = Response;
}

#[derive(Clone, Deserialize)]
struct RequestSpec {
    path: String,
    token: Option<String>,
}

struct Traffic {
    requests: Vec<RequestSpec>,
}
impl Service for Traffic {
    const NAME: &'static str = "gateway/traffic";
}

struct Limiter {
    limit: u32,
}
impl Service for Limiter {
    const NAME: &'static str = "gateway/limiter";
}

struct TimerSeat {
    ctx: Context,
}
impl Service for TimerSeat {
    const NAME: &'static str = "gateway/timer-seat";
}

#[derive(Clone, Deserialize)]
struct GuardConfig {
    lock_auth: bool,
}
struct GuardPlugin;
impl Plugin for GuardPlugin {
    type Config = GuardConfig;
    type Input = GuardConfig;
    type PrepareError = Infallible;
    type ApplyError = DemoError;

    fn prepare(&self, config: Self::Config) -> Result<Self::Input, Self::PrepareError> {
        Ok(config)
    }

    async fn apply(&self, _ctx: Context, input: &Self::Input) -> Result<(), Self::ApplyError> {
        println!("  guard policy loaded: lock_auth={}", input.lock_auth);
        Ok(())
    }
}

#[derive(Clone, Deserialize)]
struct DeadlineConfig {
    fast_ms: u64,
}
struct DeadlinePlugin;
impl Plugin for DeadlinePlugin {
    type Config = DeadlineConfig;
    type Input = DeadlineConfig;
    type PrepareError = Infallible;
    type ApplyError = DemoError;

    fn prepare(&self, config: Self::Config) -> Result<Self::Input, Self::PrepareError> {
        Ok(config)
    }

    async fn apply(&self, ctx: Context, input: &Self::Input) -> Result<(), Self::ApplyError> {
        let _timer_seat = ctx
            .provide(Arc::new(TimerSeat { ctx: ctx.clone() }))
            .map_err(DemoError::from_display)?;
        let fast = Duration::from_millis(input.fast_ms);
        let _deadline = ctx
            .on_with::<Gate, _>(
                around::<Gate, _>(move |ctx: Context, req: Request, next: Next<Gate>| {
                    let slow = req.path == "/slow";
                    let delay = if slow { Duration::ZERO } else { fast };
                    async move {
                        let work = async move {
                            if slow {
                                future::pending::<()>().await;
                                unreachable!("pending slow work cannot complete");
                            }
                            next.call(req).await.map_err(DemoError::from_display)
                        };
                        let timeout = ctx.timeout(delay, work).map_err(DemoError::from_display)?;
                        match timeout.await {
                            Ok(TimeoutOutcome::Completed(result)) => {
                                println!("  timeout completed");
                                result
                            }
                            Ok(TimeoutOutcome::Elapsed) => {
                                println!("  timeout elapsed");
                                Ok(Response::new(504, "deadline elapsed"))
                            }
                            Err(_) => {
                                println!("  timeout cancelled");
                                Ok(Response::new(503, "deadline cancelled"))
                            }
                        }
                    }
                }),
                ListenerOptions::default().global(),
            )
            .map_err(DemoError::from_display)?;
        Ok(())
    }
}

#[derive(Clone, Deserialize)]
struct ListenerConfig {
    requests: Vec<RequestSpec>,
}
struct ListenerPlugin;
impl Plugin for ListenerPlugin {
    type Config = ListenerConfig;
    type Input = ListenerConfig;
    type PrepareError = Infallible;
    type ApplyError = DemoError;

    fn prepare(&self, config: Self::Config) -> Result<Self::Input, Self::PrepareError> {
        Ok(config)
    }

    async fn apply(&self, ctx: Context, input: &Self::Input) -> Result<(), Self::ApplyError> {
        let _traffic = ctx
            .provide(Arc::new(Traffic {
                requests: input.requests.clone(),
            }))
            .map_err(DemoError::from_display)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum AuthMode {
    Token,
    None,
}
#[derive(Clone, Deserialize)]
struct AuthConfig {
    mode: AuthMode,
    token: String,
}
type AuthInput = AuthConfig;
struct AuthPlugin;
impl Plugin for AuthPlugin {
    type Config = AuthConfig;
    type Input = AuthInput;
    type PrepareError = Infallible;
    type ApplyError = DemoError;

    fn prepare(&self, config: Self::Config) -> Result<Self::Input, Self::PrepareError> {
        Ok(config)
    }

    async fn apply(&self, ctx: Context, input: &Self::Input) -> Result<(), Self::ApplyError> {
        let _control = ctx
            .on_update::<AuthPlugin, _>(
                around::<AuthPlugin, _>(
                    |_ctx: Context, candidate: AuthInput, next: UpdateNext<AuthPlugin>| async move {
                        if candidate.mode == AuthMode::None {
                            return Ok::<_, InvocationFailure>(candidate);
                        }
                        next.call(candidate).await
                    },
                ),
                ListenerOptions::default(),
            )
            .map_err(DemoError::from_display)?;
        let config = input.clone();
        let _auth = ctx
            .on_with::<Gate, _>(
                around::<Gate, _>(move |_ctx: Context, req: Request, next: Next<Gate>| {
                    let config = config.clone();
                    async move {
                        if config.mode == AuthMode::Token
                            && req.token.as_deref() != Some(config.token.as_str())
                        {
                            return Ok(Response::new(401, "unauthorized"));
                        }
                        next.call(req).await
                    }
                }),
                ListenerOptions::default().global(),
            )
            .map_err(DemoError::from_display)?;
        Ok(())
    }
}

#[derive(Clone, Deserialize)]
struct RateLimitConfig {
    rpm: u32,
}
type RateInput = RateLimitConfig;
struct RateLimitPlugin;
impl Plugin for RateLimitPlugin {
    type Config = RateLimitConfig;
    type Input = RateInput;
    type PrepareError = Infallible;
    type ApplyError = DemoError;

    fn prepare(&self, config: Self::Config) -> Result<Self::Input, Self::PrepareError> {
        Ok(config)
    }

    async fn apply(&self, ctx: Context, input: &Self::Input) -> Result<(), Self::ApplyError> {
        let _control = ctx
            .on_update::<RateLimitPlugin, _>(
                mapper_sync::<RateLimitPlugin, _>(|_ctx: Context, candidate: RateInput| {
                    println!("  typed update control: ratelimit candidate admitted precommit");
                    Ok::<_, Infallible>(candidate)
                }),
                ListenerOptions::default(),
            )
            .map_err(DemoError::from_display)?;
        if input.rpm == 0 {
            return Err(DemoError("rpm must be nonzero".into()));
        }
        let publication = ctx
            .provide(Arc::new(Limiter { limit: 1 }))
            .map_err(DemoError::from_display)?;
        let limiter = Arc::new(Limiter { limit: input.rpm });
        publication
            .set(limiter.clone())
            .map_err(DemoError::from_display)?;
        println!("  exact publication: ratelimit replaced");
        let _limit = ctx
            .on_with::<Gate, _>(
                around::<Gate, _>(move |_ctx: Context, req: Request, next: Next<Gate>| {
                    let limiter = limiter.clone();
                    async move {
                        if limiter.limit == 0 {
                            return Ok(Response::new(429, "limited"));
                        }
                        next.call(req).await
                    }
                }),
                ListenerOptions::default().global(),
            )
            .map_err(DemoError::from_display)?;
        Ok(())
    }
}

#[derive(Clone, Deserialize)]
struct RoutesConfig {
    body: String,
}
struct RoutesPlugin;
impl Plugin for RoutesPlugin {
    type Config = RoutesConfig;
    type Input = RoutesConfig;
    type PrepareError = Infallible;
    type ApplyError = DemoError;

    fn prepare(&self, config: Self::Config) -> Result<Self::Input, Self::PrepareError> {
        Ok(config)
    }

    async fn apply(&self, ctx: Context, input: &Self::Input) -> Result<(), Self::ApplyError> {
        let body = if input.body == "ok" {
            "ok"
        } else {
            "configured"
        };
        let _route = ctx
            .on_with::<Route, _>(
                responder_sync::<Route, _>(move |_ctx, req: Request| {
                    let answer = (req.path == "/ok" || req.path == "/slow")
                        .then(|| Response::new(200, body));
                    Ok::<_, Infallible>(answer)
                }),
                ListenerOptions::default().global(),
            )
            .map_err(DemoError::from_display)?;
        Ok(())
    }
}

const GATEWAY_JSON: &str = r#"[
  {"name":"guard","key":null,"config":{"lock_auth":true},"disabled":false,"inject":[],"isolate":[]},
  {"name":"deadline","key":null,"config":{"fast_ms":1000},"disabled":false,"inject":[],"isolate":[]},
  {"name":"listener","key":null,"config":{"requests":[{"path":"/ok","token":"secret"},{"path":"/slow","token":"secret"},{"path":"/ok","token":"bad"}]},"disabled":false,"inject":[],"isolate":[]},
  {"name":"auth","key":null,"config":{"mode":"token","token":"secret"},"disabled":false,"inject":[],"isolate":[]},
  {"name":"ratelimit","key":null,"config":{"rpm":10},"disabled":false,"inject":[],"isolate":[]},
  {"name":"routes","key":null,"config":{"body":"ok"},"disabled":false,"inject":[],"isolate":[]},
  {"name":"deliberate-bad-config","key":null,"config":{"rpm":"unlimited"},"disabled":false,"inject":[],"isolate":[]},
  {"name":"deliberate-unknown","key":null,"config":null,"disabled":false,"inject":[],"isolate":[]}
]"#;

struct PlanIds {
    auth: EntryId,
    rate: EntryId,
    deadline: EntryId,
}

fn build_plan() -> Result<(cordis_loader::LoadPlan, PlanIds), BoxError> {
    let entries: Vec<PluginEntry> = serde_json::from_str(GATEWAY_JSON)?;
    let mut builder = LoadPlanBuilder::new();
    let mut auth = None;
    let mut rate = None;
    let mut deadline = None;
    for entry in entries {
        let name = entry.name.clone().unwrap_or_default();
        let id = builder.add_plugin(None, entry)?;
        match name.as_str() {
            "auth" => auth = Some(id.clone()),
            "ratelimit" => rate = Some(id.clone()),
            "deadline" => deadline = Some(id.clone()),
            _ => {}
        }
    }
    let plan = builder.finish()?;
    Ok((
        plan,
        PlanIds {
            auth: auth.expect("auth entry exists"),
            rate: rate.expect("ratelimit entry exists"),
            deadline: deadline.expect("deadline entry exists"),
        },
    ))
}

fn resolve(request: PluginRequest<'_>) -> Result<Option<cordis_core::PreparedPlugin>, DemoError> {
    let target =
        match request.resolve_key() {
            "guard" => prepare_plugin_json(GuardPlugin, request.config())
                .map_err(DemoError::from_display)?,
            "deadline" => prepare_plugin_json(DeadlinePlugin, request.config())
                .map_err(DemoError::from_display)?,
            "listener" => prepare_plugin_json(ListenerPlugin, request.config())
                .map_err(DemoError::from_display)?,
            "auth" => prepare_plugin_json(AuthPlugin, request.config())
                .map_err(DemoError::from_display)?,
            "ratelimit" | "deliberate-bad-config" => {
                prepare_plugin_json(RateLimitPlugin, request.config())
                    .map_err(DemoError::from_display)?
            }
            "routes" => prepare_plugin_json(RoutesPlugin, request.config())
                .map_err(DemoError::from_display)?,
            _ => return Ok(None),
        };
    Ok(Some(target))
}

fn fiber_handle_for(outcome: &LoadOutcome, id: &EntryId) -> FiberHandle {
    match outcome.entry(id).expect("plan id has one complete outcome") {
        EntryOutcome::Spawned { fiber_handle, .. } => fiber_handle.clone(),
        _ => panic!("required gateway row did not spawn"),
    }
}

async fn request(ctx: &Context, routing: Routing, request: Request) -> Result<Response, BoxError> {
    let response = ctx
        .waterfall_query::<Gate, Route, _, _, DemoError>(routing, request, |query| async move {
            match query {
                Ok(QueryOutcome::Answer(response)) => Ok(response),
                Ok(QueryOutcome::Miss) => Ok(Response::new(404, "not found")),
                Err(error) => Err(DemoError::from_display(error)),
            }
        })
        .await?;
    Ok(response)
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let root = Context::new();
    let gateway = root.with_child_scope();
    let routing = Routing::Scoped(gateway.scope());

    section("boot: immutable Loader plan");
    let (plan, ids) = build_plan()?;
    let frozen = plan.clone();
    let outcome = frozen.load(&gateway, &resolve).await;
    assert_eq!(outcome.entries().len(), 8);
    println!(
        "  loader outcomes: complete ({} rows)",
        outcome.entries().len()
    );
    for row in outcome.entries() {
        match row {
            EntryOutcome::Failed {
                resolve_key,
                failure,
                ..
            } if resolve_key == "deliberate-bad-config" => {
                println!("  resolver failure: deliberate-bad-config — {failure}");
            }
            EntryOutcome::Failed { resolve_key, .. } if resolve_key == "deliberate-unknown" => {
                println!("  unresolved row: deliberate-unknown");
            }
            _ => {}
        }
    }

    let roster: Roster = outcome.fiber_handles().cloned().collect();
    println!(
        "  roster handoff: {} successful FiberHandles",
        outcome.fiber_handles().count()
    );
    roster.report().await;

    let auth = fiber_handle_for(&outcome, &ids.auth);
    let rate = fiber_handle_for(&outcome, &ids.rate);
    let deadline = fiber_handle_for(&outcome, &ids.deadline);
    let traffic = gateway.try_service::<Traffic>()?;

    section("events: explicit scoped waterfall/query");
    let first = request(
        &gateway,
        routing.clone(),
        Request {
            path: traffic.requests[0].path.clone(),
            token: traffic.requests[0].token.clone(),
        },
    )
    .await?;
    println!("  scoped waterfall/query: {} {}", first.status, first.body);
    let _slow = request(
        &gateway,
        routing.clone(),
        Request {
            path: traffic.requests[1].path.clone(),
            token: traffic.requests[1].token.clone(),
        },
    )
    .await?;

    section("typed update: caller-owned veto policy");
    let veto = auth
        .update(PreparedChange::from_input::<AuthPlugin>(AuthInput {
            mode: AuthMode::None,
            token: "secret".into(),
        }))
        .await?;
    assert_eq!(veto, UpdateOutcome::Vetoed);
    let denied = request(
        &gateway,
        routing.clone(),
        Request {
            path: traffic.requests[2].path.clone(),
            token: traffic.requests[2].token.clone(),
        },
    )
    .await?;
    assert_eq!(denied.status, 401);
    println!("  typed update veto: auth unchanged");

    section("typed update: postcommit failure boundary");
    match rate
        .update(PreparedChange::from_input::<RateLimitPlugin>(RateInput {
            rpm: 0,
        }))
        .await
    {
        Err(UpdateError::Apply(_)) => {
            println!("  postcommit apply failure: not recoverable by control");
        }
        other => panic!("expected postcommit apply failure, got {other:?}"),
    }

    section("timer: generation cancellation");
    let timer_seat = gateway.try_service::<TimerSeat>()?;
    let cancelled = timer_seat
        .ctx
        .timeout(Duration::from_secs(60), future::pending::<()>())?;
    deadline.dispose().await?;
    assert!(cancelled.await.is_err());
    println!("  timeout cancelled");

    section("shutdown");
    roster.teardown().await;
    println!("  gateway down");
    Ok(())
}
