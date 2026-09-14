//! Contract tests for typed Plugin and Service configuration.

mod common;

use common::ordinary_fiber_count;

use cordis_core::event::observer_sync;
use cordis_core::service::ConfigResolutionError;
use cordis_core::{
    ConfigurableService, Context, Event, FiberState, InjectSpec, Plugin, PreparedPlugin, Routing,
    Scope, Service,
};
use parking_lot::Mutex;
use std::cell::Cell;
use std::convert::Infallible;
use std::future::Future;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct MinimalPlugin(Cell<u8>);

impl Plugin for MinimalPlugin {
    type Config = Rc<String>;
    type Input = Cell<usize>;
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn inject(&self) -> InjectSpec {
        InjectSpec::none()
    }

    fn prepare(&self, config: Self::Config) -> Result<Self::Input, Self::PrepareError> {
        self.0.set(self.0.get() + 1);
        Ok(Cell::new(config.len()))
    }

    fn apply(
        &self,
        _ctx: Context,
        input: &Self::Input,
    ) -> impl Future<Output = Result<(), Self::ApplyError>> + Send {
        input.set(input.get() + 1);
        std::future::ready(Ok(()))
    }
}

#[test]
fn plugin_preparation_is_synchronous_and_has_only_the_frozen_bounds() {
    let plugin = MinimalPlugin(Cell::new(0));
    let prepared = plugin
        .prepare(Rc::new("prepared".to_owned()))
        .expect("infallible preparation");

    assert_eq!(plugin.0.get(), 1);
    assert_eq!(prepared.get(), 8);
}

struct FalliblePreparation;

impl Plugin for FalliblePreparation {
    type Config = &'static str;
    type Input = ();
    type PrepareError = ConfigError;
    type ApplyError = Infallible;

    fn prepare(&self, config: Self::Config) -> Result<Self::Input, Self::PrepareError> {
        match config {
            "error" => Err(ConfigError),
            "panic" => panic!("plugin prepare panic"),
            _ => Ok(()),
        }
    }

    fn apply(
        &self,
        _ctx: Context,
        _input: &Self::Input,
    ) -> impl Future<Output = Result<(), Self::ApplyError>> + Send {
        std::future::ready(Ok(()))
    }
}

#[derive(Debug, thiserror::Error)]
#[error("configuration rejected")]
struct ConfigError;

struct Pipeline;

impl Service for Pipeline {
    const NAME: &'static str = "pipeline";
}

impl ConfigurableService for Pipeline {
    type Config = &'static str;
    type Layer = String;
    type Resolved = Vec<String>;
    type PrepareError = ConfigError;
    type ComposeError = ConfigError;

    fn prepare_config(config: Self::Config) -> Result<Self::Layer, Self::PrepareError> {
        match config {
            "error" => Err(ConfigError),
            "panic" => panic!("prepare panic"),
            value => Ok(value.to_owned()),
        }
    }

    fn compose_config<'a>(
        base: Option<&'a Self::Layer>,
        layers: impl IntoIterator<Item = &'a Self::Layer>,
        head: Option<&'a Self::Layer>,
    ) -> Result<Self::Resolved, Self::ComposeError> {
        let mut resolved = Vec::new();
        resolved.extend(base.cloned());
        for layer in layers {
            if layer == "error" {
                return Err(ConfigError);
            }
            if layer == "panic" {
                panic!("compose panic");
            }
            resolved.push(layer.clone());
        }
        resolved.extend(head.cloned());
        Ok(resolved)
    }
}

struct ImpostorPipeline;

impl Service for ImpostorPipeline {
    const NAME: &'static str = Pipeline::NAME;
}

impl ConfigurableService for ImpostorPipeline {
    type Config = ();
    type Layer = usize;
    type Resolved = usize;
    type PrepareError = Infallible;
    type ComposeError = Infallible;

    fn prepare_config(_config: Self::Config) -> Result<Self::Layer, Self::PrepareError> {
        Ok(0)
    }

    fn compose_config<'a>(
        base: Option<&'a Self::Layer>,
        layers: impl IntoIterator<Item = &'a Self::Layer>,
        head: Option<&'a Self::Layer>,
    ) -> Result<Self::Resolved, Self::ComposeError> {
        Ok(base.into_iter().chain(layers).chain(head).copied().sum())
    }
}

#[test]
fn service_preparation_and_composition_are_synchronous_and_typed() {
    assert_eq!(Pipeline::prepare_config("outer").unwrap(), "outer");
    assert!(Pipeline::prepare_config("error").is_err());
    assert!(std::panic::catch_unwind(|| Pipeline::prepare_config("panic")).is_err());

    let ctx = Context::new()
        .with_intercept::<Pipeline>("outer".to_owned())
        .with_intercept::<Pipeline>("inner".to_owned());
    let base = "base".to_owned();
    let head = "head".to_owned();
    assert_eq!(
        ctx.resolve_config::<Pipeline>(Some(&base), Some(&head))
            .unwrap(),
        ["base", "outer", "inner", "head"]
    );

    let compose_error = ctx.with_intercept::<Pipeline>("error".to_owned());
    assert!(matches!(
        compose_error.resolve_config::<Pipeline>(None, None),
        Err(ConfigResolutionError::Compose(ConfigError))
    ));
    let compose_panic = ctx.with_intercept::<Pipeline>("panic".to_owned());
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            compose_panic.resolve_config::<Pipeline>(None, None)
        }))
        .is_err()
    );

    let mismatch = ctx.with_intercept::<ImpostorPipeline>(1);
    assert!(matches!(
        mismatch.resolve_config::<Pipeline>(None, None),
        Err(ConfigResolutionError::ContractMismatch {
            service: "pipeline"
        })
    ));
}

#[test]
fn plugin_preparation_failure_and_panic_precede_all_runtime_state() {
    let ctx = Context::new();
    assert!(FalliblePreparation.prepare("error").is_err());
    assert!(std::panic::catch_unwind(|| FalliblePreparation.prepare("panic")).is_err());
    assert_eq!(ordinary_fiber_count(&ctx), 0);
}

struct ReentrantLayer(Context);
struct ReentrantService;

impl Service for ReentrantService {
    const NAME: &'static str = "reentrant";
}

impl ConfigurableService for ReentrantService {
    type Config = Context;
    type Layer = ReentrantLayer;
    type Resolved = Vec<String>;
    type PrepareError = Infallible;
    type ComposeError = ConfigError;

    fn prepare_config(config: Self::Config) -> Result<Self::Layer, Self::PrepareError> {
        Ok(ReentrantLayer(config))
    }

    fn compose_config<'a>(
        _base: Option<&'a Self::Layer>,
        layers: impl IntoIterator<Item = &'a Self::Layer>,
        _head: Option<&'a Self::Layer>,
    ) -> Result<Self::Resolved, Self::ComposeError> {
        let mut nested = Vec::new();
        for ReentrantLayer(ctx) in layers {
            nested = ctx
                .with_intercept::<Pipeline>("nested".to_owned())
                .resolve_config::<Pipeline>(None, None)
                .unwrap();
        }
        Ok(nested)
    }
}

#[test]
fn service_composition_can_reenter_context_configuration() {
    let ctx = Context::new();
    let derived = ctx.with_intercept::<ReentrantService>(ReentrantLayer(ctx.clone()));
    assert_eq!(
        derived
            .resolve_config::<ReentrantService>(None, None)
            .unwrap(),
        ["nested"]
    );
}

struct DeclaredOnce {
    names: Arc<AtomicUsize>,
    injects: Arc<AtomicUsize>,
    applied: Arc<AtomicUsize>,
}

impl Plugin for DeclaredOnce {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn name(&self) -> std::borrow::Cow<'_, str> {
        self.names.fetch_add(1, Ordering::SeqCst);
        "declared-once".into()
    }

    fn inject(&self) -> InjectSpec {
        self.injects.fetch_add(1, Ordering::SeqCst);
        InjectSpec::none()
    }
    fn prepare(&self, _config: Self::Config) -> Result<Self::Input, Self::PrepareError> {
        Ok(())
    }

    fn apply(
        &self,
        _ctx: Context,
        _input: &Self::Input,
    ) -> impl Future<Output = Result<(), Self::ApplyError>> + Send {
        self.applied.fetch_add(1, Ordering::SeqCst);
        std::future::ready(Ok(()))
    }
}

#[tokio::test]
async fn sealing_materializes_declarations_once_before_lifecycle() {
    let ctx = Context::new();
    let names = Arc::new(AtomicUsize::new(0));
    let injects = Arc::new(AtomicUsize::new(0));
    let applied = Arc::new(AtomicUsize::new(0));
    let plugin = DeclaredOnce {
        names: names.clone(),
        injects: injects.clone(),
        applied: applied.clone(),
    };

    let sealed = PreparedPlugin::from_input(plugin, ());
    assert_eq!(
        names.load(Ordering::SeqCst),
        1,
        "sealing materializes name() exactly once"
    );
    assert_eq!(
        injects.load(Ordering::SeqCst),
        1,
        "sealing materializes inject() exactly once"
    );
    assert_eq!(applied.load(Ordering::SeqCst), 0);
    assert_eq!(ordinary_fiber_count(&ctx), 0, "sealing allocates no Fiber");

    let fiber_handle = ctx.spawn(sealed).await.unwrap();
    let identity = fiber_handle.id();
    fiber_handle.ready().await.unwrap();
    assert_eq!(
        fiber_handle.id(),
        identity,
        "ready preserves Fiber identity"
    );
    assert_eq!(
        names.load(Ordering::SeqCst),
        1,
        "lifecycle never calls name() again"
    );
    assert_eq!(
        injects.load(Ordering::SeqCst),
        1,
        "lifecycle never calls inject() again"
    );
    assert_eq!(applied.load(Ordering::SeqCst), 1);
}

struct ResolvesPipeline {
    seen: Arc<Mutex<Vec<String>>>,
}

impl Plugin for ResolvesPipeline {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn inject(&self) -> InjectSpec {
        InjectSpec::none().require_configured::<Pipeline>("plugin".to_owned())
    }

    fn prepare(&self, _config: Self::Config) -> Result<Self::Input, Self::PrepareError> {
        Ok(())
    }

    fn apply(
        &self,
        ctx: Context,
        _input: &Self::Input,
    ) -> impl Future<Output = Result<(), Self::ApplyError>> + Send {
        *self.seen.lock() = ctx.resolve_config::<Pipeline>(None, None).unwrap();
        std::future::ready(Ok(()))
    }
}

#[tokio::test]
async fn inject_overlay_replaces_or_clears_one_normalized_service_row() {
    let ctx = Context::new();
    let _ = ctx.provide(Arc::new(Pipeline)).unwrap();

    let replaced = Arc::new(Mutex::new(Vec::new()));
    let plugin = ResolvesPipeline {
        seen: replaced.clone(),
    };
    let sealed = PreparedPlugin::from_input(plugin, ()).with_inject_overlay(
        InjectSpec::none().require_configured::<Pipeline>("loader".to_owned()),
    );
    ctx.spawn(sealed).await.unwrap().ready().await.unwrap();
    assert_eq!(*replaced.lock(), ["loader"]);

    let cleared = Arc::new(Mutex::new(Vec::new()));
    let plugin = ResolvesPipeline {
        seen: cleared.clone(),
    };
    let sealed = PreparedPlugin::from_input(plugin, ())
        .with_inject_overlay(InjectSpec::none().require(Pipeline::NAME));
    ctx.spawn(sealed).await.unwrap().ready().await.unwrap();
    assert!(cleared.lock().is_empty());
}

struct DeclaresTargets {
    spec: InjectSpec,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Plugin for DeclaresTargets {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn inject(&self) -> InjectSpec {
        self.spec.clone()
    }

    fn prepare(&self, _config: ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn apply(
        &self,
        ctx: Context,
        _prepared: &(),
    ) -> impl Future<Output = Result<(), Infallible>> + Send {
        let resolved = ctx
            .resolve_config::<Pipeline>(None, None)
            .expect("the configured layer composes");
        self.seen.lock().extend(resolved);
        std::future::ready(Ok(()))
    }
}

struct Alpha;
impl Service for Alpha {
    const NAME: &'static str = "alpha";
}

struct Zeta;
impl Service for Zeta {
    const NAME: &'static str = "zeta";
}

#[tokio::test]
async fn inject_normalization_erases_order_and_duplicates_from_dependency_targets() {
    let left = InjectSpec::none()
        .require("zeta")
        .require_configured::<Pipeline>("superseded".to_owned())
        .require("zeta")
        .require_configured::<Pipeline>("final".to_owned())
        .require("alpha");
    let right = InjectSpec::none()
        .require("alpha")
        .require_configured::<Pipeline>("final".to_owned())
        .require("zeta");

    let ctx = Context::new();
    let left_seen = Arc::new(Mutex::new(Vec::new()));
    let right_seen = Arc::new(Mutex::new(Vec::new()));
    let left = ctx
        .spawn(PreparedPlugin::from_input(
            DeclaresTargets {
                spec: left,
                seen: left_seen.clone(),
            },
            (),
        ))
        .await
        .unwrap();
    let right = ctx
        .spawn(PreparedPlugin::from_input(
            DeclaresTargets {
                spec: right,
                seen: right_seen.clone(),
            },
            (),
        ))
        .await
        .unwrap();

    // the dependency-target identity: same names, order- and
    // duplicate-free, for both declaration orders
    assert_eq!(left.pending_missing(), right.pending_missing());
    assert_eq!(
        left.pending_missing(),
        ["alpha", Pipeline::NAME, "zeta"].map(str::to_owned)
    );

    // the normalized target retains the winning configured row: had
    // normalization dropped it, apply would compose without "final"
    let _ = ctx.provide(Arc::new(Pipeline)).unwrap();
    let _ = ctx.provide(Arc::new(Alpha)).unwrap();
    let _ = ctx.provide(Arc::new(Zeta)).unwrap();
    left.ready().await.unwrap();
    right.ready().await.unwrap();
    assert_eq!(*left_seen.lock(), ["final".to_owned()]);
    assert_eq!(*right_seen.lock(), ["final".to_owned()]);
}

#[tokio::test]
async fn inject_overlay_cannot_select_a_service_realm() {
    let root = Context::new();
    let _ = root.provide(Arc::new(Pipeline)).unwrap();
    let isolated = root.with_isolated_service(Pipeline::NAME);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sealed = PreparedPlugin::from_input(ResolvesPipeline { seen: seen.clone() }, ())
        .with_inject_overlay(
            InjectSpec::none().require_configured::<Pipeline>("isolated".to_owned()),
        );

    let fiber_handle = isolated.spawn(sealed).await.unwrap();
    assert_eq!(fiber_handle.state(), FiberState::Pending);
    assert_eq!(fiber_handle.pending_missing(), [Pipeline::NAME.to_owned()]);
    assert!(seen.lock().is_empty());

    let _ = isolated.provide(Arc::new(Pipeline)).unwrap();
    fiber_handle.ready().await.unwrap();
    assert_eq!(*seen.lock(), ["isolated"]);
}

struct AxisMarker;
impl Service for AxisMarker {
    const NAME: &'static str = "axis-marker";
}

struct AxisPing;
impl Event for AxisPing {
    const NAME: &'static str = "axis-ping";
    type Args = ();
    type Output = ();
}

struct AxisProbe {
    derived: Arc<Mutex<Option<Context>>>,
    scope: Arc<Mutex<Option<Scope>>>,
    hits: Arc<AtomicUsize>,
}

impl Plugin for AxisProbe {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, _config: ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn apply(
        &self,
        ctx: Context,
        _prepared: &(),
    ) -> impl Future<Output = Result<(), Infallible>> + Send {
        let derived = ctx.with_intercept::<Pipeline>("axis".to_owned());
        let hits = self.hits.clone();
        let _listener = derived
            .on::<AxisPing, _>(observer_sync(move |_, ()| -> Result<(), Infallible> {
                hits.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }))
            .unwrap();
        let _ = ctx.provide(Arc::new(AxisMarker)).unwrap();
        *self.scope.lock() = Some(ctx.scope());
        *self.derived.lock() = Some(derived);
        std::future::ready(Ok(()))
    }
}

#[tokio::test]
async fn intercept_derivation_changes_no_other_context_axis() {
    let root = Context::new();
    let origin = root.with_isolated_service(AxisMarker::NAME);
    let derived = Arc::new(Mutex::new(None));
    let scope = Arc::new(Mutex::new(None));
    let hits = Arc::new(AtomicUsize::new(0));
    let sealed = PreparedPlugin::from_input(
        AxisProbe {
            derived: derived.clone(),
            scope: scope.clone(),
            hits: hits.clone(),
        },
        (),
    );
    let fiber_handle = origin.spawn(sealed).await.unwrap();
    fiber_handle.ready().await.unwrap();

    let derived = derived.lock().take().unwrap();
    assert!(derived.try_service::<AxisMarker>().is_ok());
    assert!(root.try_service::<AxisMarker>().is_err());

    let scope = scope.lock().take().unwrap();
    root.emit::<AxisPing>(Routing::Scoped(scope), ())
        .await
        .unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    fiber_handle.dispose().await.unwrap();
    assert!(matches!(
        derived.effect_sync(|| {}),
        Err(cordis_core::effect::EffectRegistrationError::InactiveContext)
    ));
}

struct ConfigRecorder {
    seen: Arc<Mutex<Vec<String>>>,
}

impl Plugin for ConfigRecorder {
    type Config = String;
    type Input = String;
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, config: String) -> Result<String, Infallible> {
        Ok(config)
    }

    fn apply(
        &self,
        _ctx: Context,
        input: &String,
    ) -> impl Future<Output = Result<(), Infallible>> + Send {
        self.seen.lock().push(input.clone());
        std::future::ready(Ok(()))
    }
}

struct ForeignPlugin;

impl Plugin for ForeignPlugin {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, _config: ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn apply(
        &self,
        _ctx: Context,
        _prepared: &(),
    ) -> impl Future<Output = Result<(), Infallible>> + Send {
        std::future::ready(Ok(()))
    }
}

#[tokio::test]
async fn update_installs_the_consumed_candidate_and_restarts_onto_it() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(
            ConfigRecorder { seen: seen.clone() },
            "v1".to_owned(),
        ))
        .await
        .unwrap();
    fiber_handle.ready().await.unwrap();

    let outcome = fiber_handle
        .update(cordis_core::PreparedChange::from_input::<ConfigRecorder>(
            "v2".to_owned(),
        ))
        .await
        .unwrap();

    assert_eq!(
        outcome,
        cordis_core::UpdateOutcome::Committed(cordis_core::FiberState::Active)
    );
    assert_eq!(
        *seen.lock(),
        ["v1".to_owned(), "v2".to_owned()],
        "the restart applies the consumed candidate, not the sealed spawn-time value"
    );
}

#[tokio::test]
async fn a_foreign_contract_update_is_refused_precommit_with_state_intact() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let fiber_handle = ctx
        .spawn(PreparedPlugin::from_input(
            ConfigRecorder { seen: seen.clone() },
            "v1".to_owned(),
        ))
        .await
        .unwrap();
    fiber_handle.ready().await.unwrap();

    let error = fiber_handle
        .update(cordis_core::PreparedChange::from_input::<ForeignPlugin>(()))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        cordis_core::lifecycle::UpdateError::PluginContractMismatch
    ));
    assert_eq!(
        *seen.lock(),
        ["v1".to_owned()],
        "a refused candidate never reaches apply"
    );

    // the old state is intact: a valid candidate still installs and applies
    fiber_handle
        .update(cordis_core::PreparedChange::from_input::<ConfigRecorder>(
            "v2".to_owned(),
        ))
        .await
        .unwrap();
    assert_eq!(*seen.lock(), ["v1".to_owned(), "v2".to_owned()]);
}

#[tokio::test]
async fn era_swap_moves_behavior_onto_the_consumed_candidate() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let old = ctx
        .spawn(PreparedPlugin::from_input(
            ConfigRecorder { seen: seen.clone() },
            "v1".to_owned(),
        ))
        .await
        .unwrap();
    old.ready().await.unwrap();

    let successor = old
        .era_swap(cordis_core::PreparedChange::from_input::<ConfigRecorder>(
            "v2".to_owned(),
        ))
        .await
        .unwrap();

    assert_eq!(successor.state(), FiberState::Active);
    assert_eq!(
        *seen.lock(),
        ["v1".to_owned(), "v2".to_owned()],
        "the successor applies the consumed candidate, not the old era's value"
    );
}

#[tokio::test]
async fn era_swap_refuses_a_foreign_contract_candidate_before_any_effect() {
    let ctx = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let old = ctx
        .spawn(PreparedPlugin::from_input(
            ConfigRecorder { seen: seen.clone() },
            "v1".to_owned(),
        ))
        .await
        .unwrap();
    old.ready().await.unwrap();

    let error = old
        .era_swap(cordis_core::PreparedChange::from_input::<ForeignPlugin>(()))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        cordis_core::lifecycle::EraSwapError::PluginContractMismatch
    ));
    assert_eq!(
        old.state(),
        FiberState::Active,
        "a precommit refusal leaves the old era intact"
    );
    assert_eq!(*seen.lock(), ["v1".to_owned()]);
}

#[test]
fn declaration_panics_unwind_before_lifecycle_admission() {
    struct Panics;
    impl Plugin for Panics {
        type Config = ();
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = Infallible;

        fn name(&self) -> std::borrow::Cow<'_, str> {
            panic!("name panic")
        }

        fn prepare(&self, _config: ()) -> Result<(), Infallible> {
            Ok(())
        }

        fn apply(
            &self,
            _ctx: Context,
            _prepared: &(),
        ) -> impl Future<Output = Result<(), Infallible>> + Send {
            std::future::ready(Ok(()))
        }
    }

    let ctx = Context::new();
    assert!(std::panic::catch_unwind(|| PreparedPlugin::from_input(Panics, ())).is_err());
    assert_eq!(ordinary_fiber_count(&ctx), 0);

    struct InjectPanics;
    impl Plugin for InjectPanics {
        type Config = ();
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = Infallible;

        fn name(&self) -> std::borrow::Cow<'_, str> {
            "inject-panics".into()
        }

        fn inject(&self) -> InjectSpec {
            panic!("inject panic")
        }

        fn prepare(&self, _config: ()) -> Result<(), Infallible> {
            Ok(())
        }

        fn apply(
            &self,
            _ctx: Context,
            _prepared: &(),
        ) -> impl Future<Output = Result<(), Infallible>> + Send {
            std::future::ready(Ok(()))
        }
    }

    assert!(std::panic::catch_unwind(|| PreparedPlugin::from_input(InjectPanics, ())).is_err());
    assert_eq!(ordinary_fiber_count(&ctx), 0);
}
