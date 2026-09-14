//! Issue 31 contract: typed Event roles, explicit Scope routing, and state factories.

mod common;

use cordis_core::event::{
    DispatchError, EventOperation, ListenerOptions, ListenerRole, mapper_sync, observer,
    observer_sync, responder_sync, with_state,
};
use cordis_core::{
    ConfigurableService, Context, Event, Plugin, PreparedPlugin, QueryOutcome, Routing, Service,
};
use parking_lot::Mutex;
use std::borrow::Cow;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Ping;
impl Event for Ping {
    const NAME: &'static str = "issue31/ping";
    type Args = usize;
    type Output = usize;
}

struct AliasPing;
impl Event for AliasPing {
    const NAME: &'static str = "issue31/ping";
    type Args = usize;
    type Output = usize;
}

struct SameNameDifferentContract;
impl Event for SameNameDifferentContract {
    const NAME: &'static str = "issue31/ping";
    type Args = String;
    type Output = usize;
}

struct SameNameDifferentOutput;
impl Event for SameNameDifferentOutput {
    const NAME: &'static str = "issue31/ping";
    type Args = usize;
    type Output = String;
}

#[tokio::test]
async fn scoped_routing_distinguishes_root_ancestor_self_sibling_descendant_and_global() {
    let root = Context::new();
    let parent = root.with_child_scope();
    let child = parent.with_child_scope();
    let sibling = parent.with_child_scope();
    let descendant = child.with_child_scope();
    let hits = Arc::new(Mutex::new(Vec::<&'static str>::new()));

    for (ctx, label, options) in [
        (&root, "root", ListenerOptions::default()),
        (&parent, "parent", ListenerOptions::default()),
        (&child, "child", ListenerOptions::default()),
        (&sibling, "sibling", ListenerOptions::default()),
        (&descendant, "descendant", ListenerOptions::default()),
        (&sibling, "global", ListenerOptions::default().global()),
    ] {
        let hits = hits.clone();
        ctx.on_with::<Ping, _>(
            observer_sync(move |_, _| {
                hits.lock().push(label);
                Ok::<(), Infallible>(())
            }),
            options,
        )
        .unwrap();
    }

    root.emit::<Ping>(Routing::Scoped(child.scope()), 1)
        .await
        .unwrap();
    assert_eq!(&*hits.lock(), &["root", "parent", "child", "global"]);

    hits.lock().clear();
    root.emit::<Ping>(Routing::Scoped(root.scope()), 2)
        .await
        .unwrap();
    assert_eq!(&*hits.lock(), &["root", "global"]);

    hits.lock().clear();
    root.emit::<Ping>(Routing::Unscoped, 3).await.unwrap();
    assert_eq!(
        &*hits.lock(),
        &["root", "parent", "child", "sibling", "descendant", "global"]
    );
}

#[tokio::test]
async fn foreign_scope_contract_and_role_preflight_run_before_factory_or_callback() {
    let ctx = Context::new();
    let foreign = Context::new();
    let factories = Arc::new(AtomicUsize::new(0));
    let callbacks = Arc::new(AtomicUsize::new(0));

    let factory_count = factories.clone();
    let callback_count = callbacks.clone();
    ctx.on::<Ping, _>(mapper_sync(with_state(
        move || {
            factory_count.fetch_add(1, Ordering::SeqCst);
            7usize
        },
        move |_, _state, args| {
            callback_count.fetch_add(1, Ordering::SeqCst);
            Ok::<usize, Infallible>(args + 1)
        },
    )))
    .unwrap();

    let err = ctx
        .emit::<Ping>(Routing::Scoped(foreign.scope()), 1)
        .await
        .unwrap_err();
    assert!(matches!(err, DispatchError::ForeignScope));
    assert_eq!(factories.load(Ordering::SeqCst), 0);
    assert_eq!(callbacks.load(Ordering::SeqCst), 0);

    let err = ctx
        .emit::<SameNameDifferentContract>(Routing::Unscoped, "x".to_owned())
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        DispatchError::EventContractMismatch {
            event: "issue31/ping"
        }
    ));
    assert_eq!(factories.load(Ordering::SeqCst), 0);
    assert_eq!(callbacks.load(Ordering::SeqCst), 0);

    let err = ctx.emit::<Ping>(Routing::Unscoped, 2).await.unwrap_err();
    assert!(matches!(
        err,
        DispatchError::IncompatibleRole {
            operation: EventOperation::Emit,
            role: ListenerRole::Mapper,
        }
    ));
    assert_eq!(factories.load(Ordering::SeqCst), 0);
    assert_eq!(callbacks.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn isolate_derivation_does_not_change_scope_routing() {
    let root = Context::new();
    let scoped = root.with_child_scope();
    let isolated = scoped.with_isolated_service("unrelated-service");
    let sibling = root.with_child_scope();
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();

    scoped
        .on::<Ping, _>(observer_sync(move |_, _| {
            h.fetch_add(1, Ordering::SeqCst);
            Ok::<(), Infallible>(())
        }))
        .unwrap();

    root.emit::<Ping>(Routing::Scoped(isolated.scope()), 1)
        .await
        .unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    root.emit::<Ping>(Routing::Scoped(sibling.scope()), 1)
        .await
        .unwrap();
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "isolate derivation must not create Scope ancestry"
    );
}

#[tokio::test]
async fn with_state_is_fresh_and_query_short_circuit_allocates_only_the_winner() {
    struct Ask;
    impl Event for Ask {
        const NAME: &'static str = "issue31/ask";
        type Args = usize;
        type Output = usize;
    }

    let ctx = Context::new();
    let first_factories = Arc::new(AtomicUsize::new(0));
    let later_factories = Arc::new(AtomicUsize::new(0));

    let first = first_factories.clone();
    ctx.on::<Ask, _>(responder_sync(with_state(
        move || first.fetch_add(1, Ordering::SeqCst),
        |_, state, args| Ok::<Option<usize>, Infallible>(Some(state + args)),
    )))
    .unwrap();

    let later = later_factories.clone();
    ctx.on::<Ask, _>(responder_sync(with_state(
        move || later.fetch_add(1, Ordering::SeqCst),
        |_, state, args| Ok::<Option<usize>, Infallible>(Some(state + args + 100)),
    )))
    .unwrap();

    assert_eq!(
        ctx.query::<Ask>(Routing::Unscoped, 10).await.unwrap(),
        QueryOutcome::Answer(10)
    );
    assert_eq!(first_factories.load(Ordering::SeqCst), 1);
    assert_eq!(later_factories.load(Ordering::SeqCst), 0);

    assert_eq!(
        ctx.query::<Ask>(Routing::Unscoped, 20).await.unwrap(),
        QueryOutcome::Answer(21)
    );
    assert_eq!(
        first_factories.load(Ordering::SeqCst),
        2,
        "fresh state per invocation"
    );
    assert_eq!(later_factories.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn state_factory_panic_is_contained_as_listener_invocation_failure() {
    let ctx = Context::new();
    ctx.on::<Ping, _>(observer_sync(with_state(
        || -> usize { panic!("factory-boom") },
        |_, _state, _| Ok::<(), Infallible>(()),
    )))
    .unwrap();

    let err = ctx.emit::<Ping>(Routing::Unscoped, 1).await.unwrap_err();
    let DispatchError::Invocation(failure) = err else {
        panic!("expected invocation failure")
    };
    assert_eq!(
        failure.kind(),
        cordis_core::event::InvocationFailureKind::Panic
    );
    assert!(failure.diagnostic().contains("factory-boom"));
}

struct RealmProbe(&'static str);
impl Service for RealmProbe {
    const NAME: &'static str = "issue31/realm-probe";
}

struct ConfigProbe;
impl Service for ConfigProbe {
    const NAME: &'static str = "issue31/config-probe";
}
impl ConfigurableService for ConfigProbe {
    type Config = usize;
    type Layer = usize;
    type Resolved = usize;
    type PrepareError = Infallible;
    type ComposeError = Infallible;

    fn prepare_config(config: usize) -> Result<usize, Infallible> {
        Ok(config)
    }

    fn compose_config<'a>(
        base: Option<&'a usize>,
        layers: impl IntoIterator<Item = &'a usize>,
        head: Option<&'a usize>,
    ) -> Result<usize, Infallible> {
        Ok(base.into_iter().chain(layers).chain(head).copied().sum())
    }
}

#[tokio::test]
async fn callback_context_is_the_registration_context_and_derivations_preserve_scope() {
    let root = Context::new();
    let _root_publication = root.provide(Arc::new(RealmProbe("root"))).unwrap();
    let registration = root
        .with_child_scope()
        .with_isolated_service(RealmProbe::NAME)
        .with_intercept::<ConfigProbe>(7);
    let _registration_publication = registration
        .provide(Arc::new(RealmProbe("registration")))
        .unwrap();

    let routed = registration.with_intercept::<ConfigProbe>(100);
    let seen = Arc::new(Mutex::new(None::<(&'static str, usize)>));
    let callback_seen = seen.clone();
    registration
        .on::<Ping, _>(observer_sync(move |ctx: Context, _| {
            let realm = ctx.try_service::<RealmProbe>().unwrap();
            let config = ctx.resolve_config::<ConfigProbe>(None, None).unwrap();
            *callback_seen.lock() = Some((realm.0, config));
            Ok::<(), Infallible>(())
        }))
        .unwrap();

    root.emit::<Ping>(Routing::Scoped(routed.scope()), 1)
        .await
        .unwrap();
    assert_eq!(*seen.lock(), Some(("registration", 7)));
}

struct EffectAttributionPlugin {
    cleaned: Arc<AtomicUsize>,
}

impl Plugin for EffectAttributionPlugin {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = std::io::Error;

    fn name(&self) -> Cow<'_, str> {
        Cow::Borrowed("issue31-effect-attribution")
    }
    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _: &()) -> Result<(), std::io::Error> {
        let cleaned = self.cleaned.clone();
        ctx.on::<Ping, _>(observer_sync(move |registration_ctx: Context, _| {
            let cleaned = cleaned.clone();
            registration_ctx
                .effect_sync(move || {
                    cleaned.fetch_add(1, Ordering::SeqCst);
                })
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            Ok::<(), std::io::Error>(())
        }))
        .map_err(|error| std::io::Error::other(error.to_string()))?;
        Ok(())
    }
}

#[tokio::test]
async fn callback_context_keeps_the_registering_fiber_for_new_effects() {
    let root = Context::new();
    let cleaned = Arc::new(AtomicUsize::new(0));
    let fiber_handle = root
        .spawn(PreparedPlugin::from_input(
            EffectAttributionPlugin {
                cleaned: cleaned.clone(),
            },
            (),
        ))
        .await
        .unwrap();

    root.emit::<Ping>(Routing::Unscoped, 1).await.unwrap();
    assert_eq!(cleaned.load(Ordering::SeqCst), 0);
    fiber_handle.dispose().await.unwrap();
    assert_eq!(cleaned.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn losing_once_claim_never_constructs_state() {
    let root = Context::new();
    let factories = Arc::new(AtomicUsize::new(0));
    let callbacks = Arc::new(AtomicUsize::new(0));
    let f = factories.clone();
    let c = callbacks.clone();
    root.on_with::<Ping, _>(
        observer_sync(with_state(
            move || {
                f.fetch_add(1, Ordering::SeqCst);
            },
            move |_, (), _| {
                c.fetch_add(1, Ordering::SeqCst);
                Ok::<(), Infallible>(())
            },
        )),
        ListenerOptions::default().once(),
    )
    .unwrap();

    let left = root.emit::<Ping>(Routing::Unscoped, 1);
    let right = root.emit::<Ping>(Routing::Unscoped, 2);
    let (left, right) = tokio::join!(left, right);
    left.unwrap();
    right.unwrap();
    assert_eq!(factories.load(Ordering::SeqCst), 1);
    assert_eq!(callbacks.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn event_identity_is_name_plus_contract_not_event_marker_type() {
    let ctx = Context::new();
    let hits = Arc::new(AtomicUsize::new(0));
    let callback_hits = hits.clone();
    ctx.on::<Ping, _>(observer_sync(move |_, _| {
        callback_hits.fetch_add(1, Ordering::SeqCst);
        Ok::<(), Infallible>(())
    }))
    .unwrap();

    ctx.emit::<AliasPing>(Routing::Unscoped, 1).await.unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn all_preflight_failures_leave_once_claim_and_state_factory_untouched() {
    // Foreign Scope.
    let ctx = Context::new();
    let foreign = Context::new();
    let foreign_factories = Arc::new(AtomicUsize::new(0));
    let count = foreign_factories.clone();
    ctx.on_with::<Ping, _>(
        observer_sync(with_state(
            move || {
                count.fetch_add(1, Ordering::SeqCst);
            },
            |_, (), _| Ok::<(), Infallible>(()),
        )),
        ListenerOptions::default().once(),
    )
    .unwrap();
    assert!(matches!(
        ctx.emit::<Ping>(Routing::Scoped(foreign.scope()), 1).await,
        Err(DispatchError::ForeignScope)
    ));
    assert_eq!(foreign_factories.load(Ordering::SeqCst), 0);
    ctx.emit::<Ping>(Routing::Unscoped, 1).await.unwrap();
    assert_eq!(foreign_factories.load(Ordering::SeqCst), 1);

    // Contract mismatch.
    let ctx = Context::new();
    let contract_factories = Arc::new(AtomicUsize::new(0));
    let count = contract_factories.clone();
    ctx.on_with::<Ping, _>(
        observer_sync(with_state(
            move || {
                count.fetch_add(1, Ordering::SeqCst);
            },
            |_, (), _| Ok::<(), Infallible>(()),
        )),
        ListenerOptions::default().once(),
    )
    .unwrap();
    assert!(matches!(
        ctx.emit::<SameNameDifferentContract>(Routing::Unscoped, "bad".to_owned())
            .await,
        Err(DispatchError::EventContractMismatch { .. })
    ));
    assert_eq!(contract_factories.load(Ordering::SeqCst), 0);
    ctx.emit::<Ping>(Routing::Unscoped, 1).await.unwrap();
    assert_eq!(contract_factories.load(Ordering::SeqCst), 1);

    // Role mismatch.
    let ctx = Context::new();
    let role_factories = Arc::new(AtomicUsize::new(0));
    let count = role_factories.clone();
    ctx.on_with::<Ping, _>(
        mapper_sync(with_state(
            move || {
                count.fetch_add(1, Ordering::SeqCst);
            },
            |_, (), value| Ok::<usize, Infallible>(value + 1),
        )),
        ListenerOptions::default().once(),
    )
    .unwrap();
    assert!(matches!(
        ctx.emit::<Ping>(Routing::Unscoped, 1).await,
        Err(DispatchError::IncompatibleRole {
            role: ListenerRole::Mapper,
            ..
        })
    ));
    assert_eq!(role_factories.load(Ordering::SeqCst), 0);
    let output = ctx
        .waterfall::<Ping, _, _, _>(Routing::Unscoped, 1, |value| async move {
            Ok::<usize, Infallible>(value)
        })
        .await
        .unwrap();
    assert_eq!(output, 2);
    assert_eq!(role_factories.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn callback_context_keeps_registration_scope_for_nested_effects() {
    struct Nested;
    impl Event for Nested {
        const NAME: &'static str = "issue31/nested-scope";
        type Args = ();
        type Output = ();
    }

    let root = Context::new();
    let registration = root.with_child_scope();
    let sibling = root.with_child_scope();
    let nested_hits = Arc::new(AtomicUsize::new(0));
    let callback_hits = nested_hits.clone();

    registration
        .on_with::<Ping, _>(
            observer_sync(move |handed: Context, _| {
                let callback_hits = callback_hits.clone();
                handed
                    .on::<Nested, _>(observer_sync(move |_, ()| {
                        callback_hits.fetch_add(1, Ordering::SeqCst);
                        Ok::<(), Infallible>(())
                    }))
                    .unwrap();
                Ok::<(), Infallible>(())
            }),
            ListenerOptions::default().once(),
        )
        .unwrap();

    root.emit::<Ping>(Routing::Unscoped, 1).await.unwrap();
    root.emit::<Nested>(Routing::Scoped(sibling.scope()), ())
        .await
        .unwrap();
    assert_eq!(nested_hits.load(Ordering::SeqCst), 0);
    root.emit::<Nested>(Routing::Scoped(registration.scope()), ())
        .await
        .unwrap();
    assert_eq!(nested_hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stateful_role_matrix_runs_only_on_compatible_operations() {
    struct Flow;
    impl Event for Flow {
        const NAME: &'static str = "issue31/role-matrix";
        type Args = String;
        type Output = String;
    }

    let notify = Context::new();
    let observer_factories = Arc::new(AtomicUsize::new(0));
    let responder_factories = Arc::new(AtomicUsize::new(0));
    let observer_count = observer_factories.clone();
    notify
        .on::<Flow, _>(observer_sync(with_state(
            move || observer_count.fetch_add(1, Ordering::SeqCst),
            |_, _state, _| Ok::<(), Infallible>(()),
        )))
        .unwrap();
    let responder_count = responder_factories.clone();
    notify
        .on::<Flow, _>(responder_sync(with_state(
            move || responder_count.fetch_add(1, Ordering::SeqCst),
            |_, state, value: String| Ok::<_, Infallible>(Some(format!("{value}:{state}"))),
        )))
        .unwrap();

    notify
        .emit_parallel::<Flow>(Routing::Unscoped, "notify".to_owned())
        .await
        .unwrap();
    assert_eq!(observer_factories.load(Ordering::SeqCst), 1);
    assert_eq!(responder_factories.load(Ordering::SeqCst), 1);
    assert_eq!(
        notify
            .query::<Flow>(Routing::Unscoped, "query".to_owned())
            .await
            .unwrap(),
        QueryOutcome::Answer("query:1".to_owned())
    );
    assert_eq!(observer_factories.load(Ordering::SeqCst), 2);
    assert_eq!(responder_factories.load(Ordering::SeqCst), 2);

    let waterfall = Context::new();
    let mapper_factories = Arc::new(AtomicUsize::new(0));
    let around_factories = Arc::new(AtomicUsize::new(0));
    let mapper_count = mapper_factories.clone();
    waterfall
        .on::<Flow, _>(mapper_sync(with_state(
            move || mapper_count.fetch_add(1, Ordering::SeqCst),
            |_, state, value: String| Ok::<_, Infallible>(format!("{value}-m{state}")),
        )))
        .unwrap();
    let around_count = around_factories.clone();
    waterfall
        .on::<Flow, _>(cordis_core::event::around(with_state(
            move || around_count.fetch_add(1, Ordering::SeqCst),
            |_, state, value: String, next: cordis_core::event::Next<Flow>| async move {
                let output = next.call(format!("{value}-a{state}")).await?;
                Ok::<_, cordis_core::event::InvocationFailure>(format!("{output}-post"))
            },
        )))
        .unwrap();

    let output = waterfall
        .waterfall::<Flow, _, _, _>(Routing::Unscoped, "start".to_owned(), |value| async move {
            Ok::<_, Infallible>(format!("{value}-tail"))
        })
        .await
        .unwrap();
    assert_eq!(output, "start-m0-a0-tail-post");
    assert_eq!(mapper_factories.load(Ordering::SeqCst), 1);
    assert_eq!(around_factories.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn scope_skip_does_not_construct_state() {
    let root = Context::new();
    let child = root.with_child_scope();
    let sibling = root.with_child_scope();
    let factories = Arc::new(AtomicUsize::new(0));
    let count = factories.clone();
    sibling
        .on::<Ping, _>(observer_sync(with_state(
            move || {
                count.fetch_add(1, Ordering::SeqCst);
            },
            |_, (), _| Ok::<(), Infallible>(()),
        )))
        .unwrap();

    root.emit::<Ping>(Routing::Scoped(child.scope()), 1)
        .await
        .unwrap();
    assert_eq!(factories.load(Ordering::SeqCst), 0);
    root.emit::<Ping>(Routing::Unscoped, 1).await.unwrap();
    assert_eq!(factories.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn state_factory_and_callback_run_outside_framework_locks() {
    struct Nested;
    impl Event for Nested {
        const NAME: &'static str = "issue31/reentrant-nested";
        type Args = ();
        type Output = ();
    }

    let root = Context::new();
    let from_factory = root.clone();
    let from_callback = root.clone();
    root.on::<Ping, _>(observer_sync(with_state(
        move || {
            from_factory
                .on::<Nested, _>(observer_sync(|_, ()| Ok::<(), Infallible>(())))
                .unwrap();
        },
        move |_, (), _| {
            from_callback
                .on::<Nested, _>(observer_sync(|_, ()| Ok::<(), Infallible>(())))
                .unwrap();
            Ok::<(), Infallible>(())
        },
    )))
    .unwrap();

    let completed = common::bounded(1_000, root.emit::<Ping>(Routing::Unscoped, 1))
        .await
        .expect("factory/callback re-entry deadlocked under a framework lock");
    completed.unwrap();
}

struct ClaimedClosePlugin {
    started: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    release: Arc<tokio::sync::Notify>,
    late_registration_refused: Arc<AtomicUsize>,
}

impl Plugin for ClaimedClosePlugin {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = std::io::Error;

    fn name(&self) -> Cow<'_, str> {
        Cow::Borrowed("issue31-claimed-close")
    }

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _: &()) -> Result<(), std::io::Error> {
        let started = self.started.clone();
        let release = self.release.clone();
        let refused = self.late_registration_refused.clone();
        ctx.on::<Ping, _>(observer(move |registration_ctx: Context, _| {
            let started = started.clone();
            let release = release.clone();
            let refused = refused.clone();
            async move {
                if let Some(started) = started.lock().take() {
                    let _ = started.send(());
                }
                release.notified().await;
                if registration_ctx.effect_sync(|| ()).is_err() {
                    refused.fetch_add(1, Ordering::SeqCst);
                }
                Ok::<(), Infallible>(())
            }
        }))
        .map_err(|error| std::io::Error::other(error.to_string()))?;
        Ok(())
    }
}

#[tokio::test]
async fn claimed_callback_continues_after_registering_fiber_closes_but_new_effect_is_refused() {
    let root = Context::new();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let refused = Arc::new(AtomicUsize::new(0));
    let fiber_handle = root
        .spawn(PreparedPlugin::from_input(
            ClaimedClosePlugin {
                started: Arc::new(Mutex::new(Some(started_tx))),
                release: release.clone(),
                late_registration_refused: refused.clone(),
            },
            (),
        ))
        .await
        .unwrap();

    let emitter = root.clone();
    let dispatch = tokio::spawn(async move { emitter.emit::<Ping>(Routing::Unscoped, 1).await });
    started_rx.await.unwrap();

    let disposed = common::bounded(1_000, fiber_handle.dispose())
        .await
        .expect("listener cleanup must not join an already-claimed callback");
    disposed.unwrap();
    release.notify_one();

    dispatch.await.unwrap().unwrap();
    assert_eq!(refused.load(Ordering::SeqCst), 1);
}

struct GlobalOwnershipPlugin {
    hits: Arc<AtomicUsize>,
}

impl Plugin for GlobalOwnershipPlugin {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = std::io::Error;

    fn name(&self) -> Cow<'_, str> {
        Cow::Borrowed("issue31-global-ownership")
    }

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    async fn apply(&self, ctx: Context, _: &()) -> Result<(), std::io::Error> {
        let hits = self.hits.clone();
        ctx.on_with::<Ping, _>(
            observer_sync(move |_, _| {
                hits.fetch_add(1, Ordering::SeqCst);
                Ok::<(), Infallible>(())
            }),
            ListenerOptions::default().global(),
        )
        .map_err(|error| std::io::Error::other(error.to_string()))?;
        Ok(())
    }
}

#[tokio::test]
async fn global_routing_does_not_change_registration_cleanup_ownership() {
    let root = Context::new();
    let sibling = root.with_child_scope();
    let hits = Arc::new(AtomicUsize::new(0));
    let fiber_handle = root
        .spawn(PreparedPlugin::from_input(
            GlobalOwnershipPlugin { hits: hits.clone() },
            (),
        ))
        .await
        .unwrap();

    root.emit::<Ping>(Routing::Scoped(sibling.scope()), 1)
        .await
        .unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    fiber_handle.dispose().await.unwrap();
    root.emit::<Ping>(Routing::Scoped(sibling.scope()), 2)
        .await
        .unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn event_contract_binding_is_runtime_local_and_checks_args_and_output() {
    let first = Context::new();
    first.emit::<Ping>(Routing::Unscoped, 1).await.unwrap();

    let output_mismatch = first
        .emit::<SameNameDifferentOutput>(Routing::Unscoped, 1)
        .await
        .unwrap_err();
    assert!(matches!(
        output_mismatch,
        DispatchError::EventContractMismatch {
            event: "issue31/ping"
        }
    ));

    let second = Context::new();
    second
        .emit::<SameNameDifferentContract>(Routing::Unscoped, "independent".to_owned())
        .await
        .unwrap();
}

#[test]
fn conflicting_registration_contract_fails_without_constructing_state() {
    let ctx = Context::new();
    ctx.on::<Ping, _>(observer_sync(|_, _| Ok::<(), Infallible>(())))
        .unwrap();

    let factories = Arc::new(AtomicUsize::new(0));
    let count = factories.clone();
    let result = ctx.on::<SameNameDifferentContract, _>(observer_sync(with_state(
        move || count.fetch_add(1, Ordering::SeqCst),
        |_, _state, _| Ok::<(), Infallible>(()),
    )));
    assert!(matches!(
        result,
        Err(
            cordis_core::event::ListenerRegistrationError::EventContractMismatch {
                event: "issue31/ping"
            }
        )
    ));
    assert_eq!(factories.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn foreign_scope_fails_before_event_contract_binding() {
    let local = Context::new();
    let foreign = Context::new();

    assert!(matches!(
        local
            .emit::<Ping>(Routing::Scoped(foreign.scope()), 1)
            .await,
        Err(DispatchError::ForeignScope)
    ));

    // If the rejected foreign routing had touched the local contract table,
    // this different same-name contract would now fail with mismatch.
    local
        .emit::<SameNameDifferentContract>(Routing::Unscoped, "first-local-binding".to_owned())
        .await
        .unwrap();
}

#[tokio::test]
async fn intercept_derivation_does_not_change_scope_routing() {
    let root = Context::new();
    let scoped = root.with_child_scope();
    let intercepted = scoped.with_intercept::<ConfigProbe>(9);
    let sibling = root.with_child_scope();
    let hits = Arc::new(AtomicUsize::new(0));
    let callback_hits = hits.clone();

    intercepted
        .on::<Ping, _>(observer_sync(move |_, _| {
            callback_hits.fetch_add(1, Ordering::SeqCst);
            Ok::<(), Infallible>(())
        }))
        .unwrap();

    // Dispatching at the pre-intercept Scope must still reach a registration
    // made through the intercepted view. A hidden child-Scope derivation would
    // turn that registration into a descendant and this assertion would fail.
    root.emit::<Ping>(Routing::Scoped(scoped.scope()), 1)
        .await
        .unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    root.emit::<Ping>(Routing::Scoped(sibling.scope()), 2)
        .await
        .unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}
