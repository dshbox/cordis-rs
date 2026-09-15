//! Public-path performance regression benchmarks for the Cordis core runtime.

use std::{
    convert::Infallible,
    hint::black_box,
    sync::Arc,
    time::{Duration, Instant},
};

use cordis_core::event::{observer_sync, responder_sync};
use cordis_core::{
    Context, Event, FiberHandle, FiberState, InjectSpec, Plugin, PreparedChange, PreparedPlugin,
    QueryOutcome, Routing, Service,
};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

const FANOUT_N: usize = 32;

#[derive(Clone, Copy)]
struct NoopPlugin;

impl Plugin for NoopPlugin {
    type Config = usize;
    type Input = usize;
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, config: usize) -> Result<usize, Infallible> {
        Ok(config)
    }

    async fn apply(&self, _ctx: Context, _input: &usize) -> Result<(), Infallible> {
        Ok(())
    }
}

struct BenchEvent;

impl Event for BenchEvent {
    const NAME: &'static str = "bench/runtime-path";
    type Args = usize;
    type Output = usize;
}

struct BenchService;

impl Service for BenchService {
    const NAME: &'static str = "bench/service";
}

#[derive(Clone, Copy)]
struct NeedsBenchService;

impl Plugin for NeedsBenchService {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn inject(&self) -> InjectSpec {
        InjectSpec::none().require(BenchService::NAME)
    }

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, _ctx: Context, (): &()) -> Result<(), Infallible> {
        Ok(())
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .build()
        .expect("benchmark Tokio runtime")
}

async fn spawn_noop(ctx: &Context, input: usize) -> FiberHandle {
    ctx.spawn(PreparedPlugin::from_input(NoopPlugin, input))
        .await
        .expect("no-op spawn")
}

async fn dispose_all(handles: &[FiberHandle]) {
    for handle in handles {
        handle.dispose().await.expect("benchmark cleanup");
    }
}

async fn ready_all(handles: &[FiberHandle], expected: FiberState) {
    for handle in handles {
        assert_eq!(
            handle.ready().await.expect("benchmark convergence"),
            expected
        );
    }
}

fn bench_spawn(c: &mut Criterion) {
    let rt = runtime();
    c.bench_function("lifecycle/spawn_initial_settle", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let ctx = Context::new();
            let mut measured = Duration::ZERO;
            for input in 0..iters {
                let prepared = PreparedPlugin::from_input(NoopPlugin, input as usize);
                let started = Instant::now();
                let handle = ctx.spawn(prepared).await.expect("benchmark spawn");
                measured += started.elapsed();
                handle.dispose().await.expect("benchmark cleanup");
            }
            measured
        });
    });
}

fn bench_ready(c: &mut Criterion) {
    let rt = runtime();
    let ctx = Context::new();
    let handle = rt.block_on(spawn_noop(&ctx, 0));

    c.bench_function("lifecycle/ready_already_quiescent", |b| {
        b.to_async(&rt).iter(|| async {
            black_box(handle.ready().await.expect("benchmark ready"));
        });
    });

    rt.block_on(handle.dispose()).expect("benchmark cleanup");
}

fn bench_events(c: &mut Criterion) {
    let rt = runtime();

    let mut emit = c.benchmark_group("event/emit");
    for listeners in [0usize, 1, FANOUT_N] {
        let ctx = Context::new();
        for _ in 0..listeners {
            let _registration = ctx
                .on::<BenchEvent, _>(observer_sync(|_, _| Ok::<(), Infallible>(())))
                .expect("benchmark listener registration");
        }
        emit.bench_with_input(
            BenchmarkId::from_parameter(listeners),
            &listeners,
            |b, _| {
                b.to_async(&rt).iter(|| async {
                    ctx.emit::<BenchEvent>(Routing::Unscoped, black_box(1))
                        .await
                        .expect("benchmark emit");
                });
            },
        );
    }
    emit.finish();

    let mut query = c.benchmark_group("event/query_all_miss");
    for listeners in [0usize, 1, FANOUT_N] {
        let ctx = Context::new();
        for _ in 0..listeners {
            let _registration = ctx
                .on::<BenchEvent, _>(responder_sync(|_, _| Ok::<Option<usize>, Infallible>(None)))
                .expect("benchmark responder registration");
        }
        query.bench_with_input(
            BenchmarkId::from_parameter(listeners),
            &listeners,
            |b, _| {
                b.to_async(&rt).iter(|| async {
                    let outcome = ctx
                        .query::<BenchEvent>(Routing::Unscoped, black_box(1))
                        .await
                        .expect("benchmark query");
                    assert_eq!(outcome, QueryOutcome::Miss);
                    black_box(outcome);
                });
            },
        );
    }
    query.finish();
}

fn bench_service_visibility(c: &mut Criterion) {
    let rt = runtime();
    let mut group = c.benchmark_group("service/publish_and_converge");

    for affected in [0usize, 1, FANOUT_N] {
        group.bench_with_input(
            BenchmarkId::from_parameter(affected),
            &affected,
            |b, &affected| {
                b.to_async(&rt).iter_custom(|iters| async move {
                    let ctx = Context::new();
                    let mut handles = Vec::with_capacity(affected);
                    for _ in 0..affected {
                        let handle = ctx
                            .spawn(PreparedPlugin::from_input(NeedsBenchService, ()))
                            .await
                            .expect("dependent spawn");
                        assert_eq!(handle.state(), FiberState::Pending);
                        handles.push(handle);
                    }

                    let mut measured = Duration::ZERO;
                    for _ in 0..iters {
                        let started = Instant::now();
                        let publication = ctx
                            .provide(Arc::new(BenchService))
                            .expect("benchmark service publication");
                        ready_all(&handles, FiberState::Active).await;
                        measured += started.elapsed();

                        publication.remove().expect("benchmark service reset");
                        ready_all(&handles, FiberState::Pending).await;
                    }

                    dispose_all(&handles).await;
                    measured
                });
            },
        );
    }
    group.finish();
}

fn bench_restart(c: &mut Criterion) {
    let rt = runtime();
    c.bench_function("lifecycle/restart", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let ctx = Context::new();
            let handle = spawn_noop(&ctx, 0).await;
            let mut measured = Duration::ZERO;
            for _ in 0..iters {
                let started = Instant::now();
                handle.restart().await.expect("benchmark restart");
                measured += started.elapsed();
            }
            handle.dispose().await.expect("benchmark cleanup");
            measured
        });
    });
}

fn bench_update(c: &mut Criterion) {
    let rt = runtime();
    c.bench_function("lifecycle/update", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let ctx = Context::new();
            let handle = spawn_noop(&ctx, 0).await;
            let mut measured = Duration::ZERO;
            for input in 1..=iters {
                let change = PreparedChange::from_input::<NoopPlugin>(input as usize);
                let started = Instant::now();
                black_box(handle.update(change).await.expect("benchmark update"));
                measured += started.elapsed();
            }
            handle.dispose().await.expect("benchmark cleanup");
            measured
        });
    });
}

fn bench_era_swap(c: &mut Criterion) {
    let rt = runtime();
    c.bench_function("lifecycle/era_swap", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let ctx = Context::new();
            let mut handle = spawn_noop(&ctx, 0).await;
            let mut measured = Duration::ZERO;
            for input in 1..=iters {
                let change = PreparedChange::from_input::<NoopPlugin>(input as usize);
                let started = Instant::now();
                let successor = handle.era_swap(change).await.expect("benchmark era swap");
                measured += started.elapsed();
                handle = successor;
            }
            handle.dispose().await.expect("benchmark cleanup");
            measured
        });
    });
}

fn benchmark_config() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(2))
        .sample_size(30)
        .noise_threshold(0.15)
}

criterion_group! {
    name = benches;
    config = benchmark_config();
    targets = bench_spawn, bench_ready, bench_events, bench_service_visibility, bench_restart,
        bench_update, bench_era_swap
}
criterion_main!(benches);
