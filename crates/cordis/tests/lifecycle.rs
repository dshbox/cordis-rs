use cordis::{
    Context, CordisError, ErrorCode, FiberState, Inject, PluginOutput, Result, plugin_async,
    plugin_sync,
};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[test]
fn dependency_arrival_and_removal_reload_plugins() -> Result<()> {
    let root = Context::new();
    let starts = Arc::new(AtomicUsize::new(0));
    let stops = Arc::new(AtomicUsize::new(0));

    let plugin = plugin_sync::<(), _>("consumer", Inject::new(["database"]), {
        let starts = starts.clone();
        let stops = stops.clone();
        move |ctx, _| {
            assert_eq!(*ctx.require::<u32>("database")?, 7);
            starts.fetch_add(1, Ordering::SeqCst);
            let stops = stops.clone();
            Ok(PluginOutput::infallible(move || {
                stops.fetch_add(1, Ordering::SeqCst);
            }))
        }
    });

    let fiber = root.plugin_default(plugin);
    assert_eq!(fiber.state(), FiberState::Pending);
    assert_eq!(starts.load(Ordering::SeqCst), 0);

    let database = root.provide("database", 7_u32)?;
    assert_eq!(fiber.state(), FiberState::Active);
    assert_eq!(starts.load(Ordering::SeqCst), 1);

    database.dispose()?;
    assert_eq!(fiber.state(), FiberState::Pending);
    assert_eq!(stops.load(Ordering::SeqCst), 1);

    let _database = root.provide("database", 7_u32)?;
    assert_eq!(fiber.state(), FiberState::Active);
    assert_eq!(starts.load(Ordering::SeqCst), 2);

    fiber.dispose()?;
    fiber.dispose()?;
    assert_eq!(fiber.state(), FiberState::Disposed);
    assert_eq!(stops.load(Ordering::SeqCst), 2);
    Ok(())
}

#[test]
fn effects_are_lifo_and_single_shot() -> Result<()> {
    let root = Context::new();
    let order = Arc::new(std::sync::Mutex::new(Vec::new()));

    for value in 1..=3 {
        let order = order.clone();
        root.effect_infallible(format!("effect {value}"), move || {
            order.lock().unwrap().push(value);
        })?;
    }
    assert_eq!(root.fiber()?.effects().len(), 3);
    root.fiber()?.dispose()?;
    assert_eq!(*order.lock().unwrap(), vec![3, 2, 1]);
    Ok(())
}

#[test]
fn isolated_scopes_are_independent_and_labels_can_be_shared() -> Result<()> {
    let root = Context::new();
    let label = root.new_isolation();
    let first = root.isolate_with("cache", label);
    let second = root.isolate_with("cache", label);
    let third = root.isolate("cache");

    let _service = first.provide("cache", String::from("shared"))?;
    assert_eq!(first.require::<String>("cache")?.as_str(), "shared");
    assert_eq!(second.require::<String>("cache")?.as_str(), "shared");
    assert!(third.get::<String>("cache")?.is_none());
    assert!(root.get::<String>("cache")?.is_none());
    Ok(())
}

#[test]
fn config_update_restarts_a_fiber() -> Result<()> {
    let root = Context::new();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let plugin = plugin_sync::<String, _>("configurable", Inject::none(), {
        let seen = seen.clone();
        move |_ctx, config| {
            seen.lock().unwrap().push(config.as_str().to_owned());
            Ok(PluginOutput::none())
        }
    });
    let fiber = root.plugin(plugin, String::from("first"));
    fiber.try_wait()?;
    fiber.update(String::from("second"))?;
    assert_eq!(*seen.lock().unwrap(), vec!["first", "second"]);
    Ok(())
}

#[test]
fn failed_startup_rolls_back_effects_and_update_can_recover() -> Result<()> {
    let root = Context::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let plugin = plugin_sync::<bool, _>("fallible", Inject::none(), {
        let calls = calls.clone();
        move |ctx, succeeds| {
            let calls = calls.clone();
            ctx.on("probe", move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            })?;
            if !*succeeds {
                return Err(cordis::CordisError::with_message(
                    cordis::ErrorCode::Plugin,
                    "startup failed",
                ));
            }
            Ok(PluginOutput::none())
        }
    });

    let fiber = root.plugin(plugin, false);
    assert_eq!(fiber.state(), FiberState::Failed);
    assert!(fiber.try_wait().is_err());
    root.emit("probe", [])?;
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    fiber.update(true)?;
    assert_eq!(fiber.state(), FiberState::Active);
    root.emit("probe", [])?;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    Ok(())
}

/// Upstream parity: update() on a Pending fiber stores the config without
/// waiting for dependencies; activation later uses the new config.
#[test]
fn update_on_pending_fiber_applies_config_on_activation() -> Result<()> {
    let root = Context::new();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let plugin = plugin_sync::<String, _>("configurable", Inject::new(["database"]), {
        let seen = seen.clone();
        move |_ctx, config| {
            seen.lock().unwrap().push(config.as_str().to_owned());
            Ok(PluginOutput::none())
        }
    });
    let fiber = root.plugin(plugin, String::from("first"));
    assert_eq!(fiber.state(), FiberState::Pending);

    fiber.update(String::from("second"))?;
    assert_eq!(fiber.state(), FiberState::Pending);
    assert!(seen.lock().unwrap().is_empty());

    let _database = root.provide("database", 7_u32)?;
    assert_eq!(fiber.state(), FiberState::Active);
    assert_eq!(*seen.lock().unwrap(), vec!["second"]);
    Ok(())
}

/// Upstream parity: update() on a Failed fiber clears the failure, retries
/// startup with the new config, and reports acceptance rather than the
/// outcome — a config that still fails leaves the fiber Failed with the new
/// error observable through error().
#[test]
fn update_on_failed_fiber_accepts_config_without_waiting() -> Result<()> {
    let root = Context::new();
    let attempts = Arc::new(AtomicUsize::new(0));
    let plugin = plugin_sync::<bool, _>("fallible", Inject::none(), {
        let attempts = attempts.clone();
        move |_, succeeds| {
            attempts.fetch_add(1, Ordering::SeqCst);
            if *succeeds {
                Ok(PluginOutput::none())
            } else {
                Err(CordisError::with_message(
                    ErrorCode::Plugin,
                    "startup failed",
                ))
            }
        }
    });

    let fiber = root.plugin(plugin, false);
    assert_eq!(fiber.state(), FiberState::Failed);

    fiber.update(false)?;
    assert_eq!(fiber.state(), FiberState::Failed);
    assert!(fiber.error().is_some());
    assert_eq!(attempts.load(Ordering::SeqCst), 2);

    fiber.update(true)?;
    assert_eq!(fiber.state(), FiberState::Active);
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    Ok(())
}

#[test]
fn availability_checks_can_be_explicitly_refreshed() -> Result<()> {
    let root = Context::new();
    let available = Arc::new(AtomicBool::new(false));
    let check = available.clone();
    let _service = root
        .reflect()
        .provide_checked("feature", 42_u32, move |_| check.load(Ordering::SeqCst))?;
    let plugin = plugin_sync::<(), _>("checked", Inject::new(["feature"]), |_, _| {
        Ok(PluginOutput::none())
    });
    let fiber = root.plugin_default(plugin);
    assert_eq!(fiber.state(), FiberState::Pending);

    available.store(true, Ordering::SeqCst);
    let refreshed = root.notify(["feature"]);
    assert_eq!(refreshed.len(), 1);
    assert_eq!(fiber.state(), FiberState::Active);
    Ok(())
}

#[test]
fn root_disposal_recursively_disposes_child_plugins() -> Result<()> {
    let root = Context::new();
    let stopped = Arc::new(AtomicUsize::new(0));
    let plugin = plugin_sync::<(), _>("child", Inject::none(), {
        let stopped = stopped.clone();
        move |_, _| {
            let stopped = stopped.clone();
            Ok(PluginOutput::infallible(move || {
                stopped.fetch_add(1, Ordering::SeqCst);
            }))
        }
    });
    let fiber = root.plugin_default(plugin);
    assert_eq!(fiber.state(), FiberState::Active);

    root.fiber()?.dispose()?;
    assert_eq!(fiber.state(), FiberState::Disposed);
    assert_eq!(stopped.load(Ordering::SeqCst), 1);
    assert!(root.registry().is_empty());
    assert_eq!(root.fiber()?.state(), FiberState::Active);
    Ok(())
}

/// Regression: plugin_value pushed the fiber into the registry before the
/// parent-effect registration; when that failed, reject() left the record
/// behind and len()/contains() misreported forever.
#[test]
fn rejected_plugin_leaves_no_registry_record() -> Result<()> {
    let root = Context::new();
    let slot = Arc::new(std::sync::Mutex::new(None));
    let slot_in_plugin = slot.clone();
    let parent = root.plugin_default(plugin_sync::<(), _>(
        "parent",
        Inject::none(),
        move |ctx, _| {
            *slot_in_plugin.lock().unwrap() = Some(ctx);
            Ok(PluginOutput::default())
        },
    ));
    parent.try_wait()?;
    parent.dispose()?;

    // The captured context belongs to a disposed fiber, so registering the
    // child plugin's parent effect fails and the child is rejected.
    let ctx = slot.lock().unwrap().take().unwrap();
    let child = ctx.plugin_default(plugin_sync::<(), _>("child", Inject::none(), |_, _| {
        Ok(PluginOutput::default())
    }));
    assert_eq!(child.state(), FiberState::Disposed);
    assert!(root.registry().is_empty());
    Ok(())
}

/// Regression: a Fiber outliving every Context must not panic on lifecycle
/// operations; notifications are simply skipped when the root is gone.
#[test]
fn dispose_orphaned_fiber_does_not_panic() -> Result<()> {
    let fiber = {
        let root = Context::new();
        let fiber = root.plugin_default(plugin_sync::<(), _>("orphan", Inject::none(), |_, _| {
            Ok(PluginOutput::default())
        }));
        fiber.try_wait()?;
        fiber
    };
    fiber.dispose()?;
    assert_eq!(fiber.state(), FiberState::Disposed);
    Ok(())
}

/// Regression: update() on the root fiber reached expect("plugin fiber") and
/// panicked; it must report an error instead.
#[test]
fn update_on_root_fiber_errors_instead_of_panicking() -> Result<()> {
    let root = Context::new();
    assert!(root.fiber()?.update(()).is_err());
    Ok(())
}

/// A manually gated future for driving apply() into a controlled park.
#[derive(Clone)]
struct Gate {
    state: Arc<std::sync::Mutex<GateState>>,
}

struct GateState {
    open: bool,
    entered: bool,
    waker: Option<std::task::Waker>,
}

impl Gate {
    fn new() -> Self {
        Self {
            state: Arc::new(std::sync::Mutex::new(GateState {
                open: false,
                entered: false,
                waker: None,
            })),
        }
    }

    fn open(&self) {
        let mut state = self.state.lock().unwrap();
        state.open = true;
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }

    fn entered(&self) -> bool {
        self.state.lock().unwrap().entered
    }
}

impl Future for Gate {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        let mut state = self.state.lock().unwrap();
        state.entered = true;
        if state.open {
            std::task::Poll::Ready(())
        } else {
            state.waker = Some(cx.waker().clone());
            std::task::Poll::Pending
        }
    }
}

/// A cross-thread dispose during a parked apply() waits for the in-flight
/// transition instead of silently dropping the disposal; same-thread
/// reentrancy (from a callback) keeps failing fast.
#[test]
fn dispose_during_loading_waits_for_the_transition() -> Result<()> {
    let root = Context::new();
    let gate = Gate::new();
    let gate_in_plugin = gate.clone();
    let plugin = plugin_async::<(), _, _>("gated", Inject::none(), move |_, _| {
        let gate = gate_in_plugin.clone();
        async move {
            gate.await;
            Ok(PluginOutput::default())
        }
    });

    // The internal/plugin event fires before the first refresh, so the fiber
    // handle is available while apply() is still parked.
    let slot = Arc::new(std::sync::Mutex::new(None));
    let slot_in_listener = slot.clone();
    root.on("internal/plugin", move |event| {
        if let Some(first) = event.args().first() {
            if let Ok(fiber) = first.downcast::<cordis::Fiber>() {
                *slot_in_listener.lock().unwrap() = Some(fiber);
            }
        }
        Ok(None)
    })?;

    let worker = std::thread::spawn({
        let root = root.clone();
        move || root.plugin_default(plugin)
    });
    for _ in 0..100 {
        if gate.entered() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(gate.entered(), "apply() never parked");
    let fiber = slot.lock().unwrap().clone().unwrap();

    // The disposer cannot return while apply() holds the transition: it
    // waits for the gate to open instead of reporting a conflict.
    let disposer = std::thread::spawn({
        let fiber = fiber.clone();
        move || fiber.dispose()
    });
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(
        !disposer.is_finished(),
        "dispose must block while the transition is in flight"
    );
    gate.open();
    disposer.join().unwrap()?;
    assert_eq!(fiber.state(), FiberState::Disposed);
    let _ = worker.join().unwrap();
    Ok(())
}

/// A panicking apply() propagates to the caller of the triggering lifecycle
/// operation; the fiber stays in Loading, poisoned internal locks recover,
/// and a later restart retries startup.
#[test]
fn panicking_plugin_state_is_recoverable() -> Result<()> {
    let root = Context::new();
    let should_panic = Arc::new(AtomicBool::new(true));
    let plugin = plugin_sync::<(), _>("panicky", Inject::none(), {
        let should_panic = should_panic.clone();
        move |_, _| {
            if should_panic.load(Ordering::SeqCst) {
                panic!("startup panic");
            }
            Ok(PluginOutput::none())
        }
    });

    let panicked =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| root.plugin_default(plugin)));
    assert!(panicked.is_err());

    let fiber = root.registry().values()[0].fibers[0].clone();
    assert_eq!(fiber.state(), FiberState::Loading);

    should_panic.store(false, Ordering::SeqCst);
    fiber.restart()?;
    assert_eq!(fiber.state(), FiberState::Active);
    Ok(())
}

/// restart() on a Failed fiber clears the failure epoch and retries startup.
#[test]
fn restart_retries_failed_startup() -> Result<()> {
    let root = Context::new();
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_in_plugin = attempts.clone();
    let fiber = root.plugin_default(plugin_sync::<(), _>(
        "flaky",
        Inject::none(),
        move |_, _| {
            if attempts_in_plugin.fetch_add(1, Ordering::SeqCst) == 0 {
                Err(CordisError::new(ErrorCode::Plugin))
            } else {
                Ok(PluginOutput::default())
            }
        },
    ));
    assert_eq!(fiber.state(), FiberState::Failed);
    fiber.restart()?;
    assert_eq!(fiber.state(), FiberState::Active);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    Ok(())
}

/// Disposing a Pending fiber goes straight to Disposed and cleans the
/// registry without running any plugin code.
#[test]
fn dispose_pending_fiber() -> Result<()> {
    let root = Context::new();
    let fiber = root.plugin_default(plugin_sync::<(), _>(
        "needy",
        Inject::new(["unavailable"]),
        |_, _| Ok(PluginOutput::default()),
    ));
    assert_eq!(fiber.state(), FiberState::Pending);
    fiber.dispose()?;
    assert_eq!(fiber.state(), FiberState::Disposed);
    assert!(root.registry().is_empty());
    Ok(())
}

/// A fiber with several dependencies stays Pending until every one is
/// provided, and unloads when any is removed.
#[test]
fn partial_dependencies_keep_fiber_pending() -> Result<()> {
    let root = Context::new();
    let fiber = root.plugin_default(plugin_sync::<(), _>(
        "multi",
        Inject::new(["a", "b"]),
        |_, _| Ok(PluginOutput::default()),
    ));
    let effect_a = root.provide_arc("a", Arc::new(1_u32))?;
    assert_eq!(fiber.state(), FiberState::Pending);
    let _effect_b = root.provide_arc("b", Arc::new(2_u32))?;
    fiber.await_idle();
    assert_eq!(fiber.state(), FiberState::Active);
    effect_a.dispose()?;
    fiber.await_idle();
    assert_eq!(fiber.state(), FiberState::Pending);
    Ok(())
}

/// Providing an occupied service name is rejected with DuplicateService.
#[test]
fn duplicate_provide_is_rejected() -> Result<()> {
    let root = Context::new();
    let _first = root.provide_arc("dup", Arc::new(1_u32))?;
    let second = root.provide_arc("dup", Arc::new(2_u32));
    assert_eq!(second.unwrap_err().code(), ErrorCode::DuplicateService);
    Ok(())
}

/// Regression: wait() only checked the stored error and reported success for
/// Pending (dependency-starved) fibers. It must report the settled state.
#[test]
fn wait_reports_pending_as_not_ready() -> Result<()> {
    let root = Context::new();
    let fiber = root.plugin_default(plugin_sync::<(), _>(
        "needy",
        Inject::new(["missing-service"]),
        |_, _| Ok(PluginOutput::default()),
    ));
    assert_eq!(fiber.state(), FiberState::Pending);
    assert!(fiber.try_wait().is_err());
    Ok(())
}

/// A Pending fiber's try_wait() error names the injected services that have
/// not resolved yet, and stops failing once they arrive.
#[test]
fn pending_try_wait_names_missing_services() -> Result<()> {
    let root = Context::new();
    let fiber = root.plugin_default(plugin_sync::<(), _>(
        "needy",
        Inject::new(["missing-svc"]),
        |_, _| Ok(PluginOutput::default()),
    ));
    assert_eq!(fiber.state(), FiberState::Pending);
    let error = fiber.try_wait().unwrap_err().to_string();
    assert_eq!(
        error,
        "fiber is not ready (state: Pending; missing services: missing-svc)"
    );

    let _service = root.provide("missing-svc", 7_u32)?;
    fiber.try_wait()?;
    Ok(())
}

/// With some dependencies already provided, the try_wait() error lists only
/// the still-unresolved names.
#[test]
fn pending_try_wait_lists_only_unresolved_services() -> Result<()> {
    let root = Context::new();
    let fiber = root.plugin_default(plugin_sync::<(), _>(
        "partly",
        Inject::new(["present-svc", "absent-svc"]),
        |_, _| Ok(PluginOutput::default()),
    ));
    let _present = root.provide("present-svc", 1_u32)?;
    assert_eq!(fiber.state(), FiberState::Pending);
    let error = fiber.try_wait().unwrap_err().to_string();
    assert!(error.contains("absent-svc"), "error was: {error}");
    assert!(!error.contains("present-svc"), "error was: {error}");
    Ok(())
}

/// Regression: restart() on the root fiber disposed every root-owned effect
/// (including all top-level plugin parent effects) and left the root stuck in
/// Pending while reporting success. It must fail without side effects.
#[test]
fn restart_on_root_fiber_errors_without_side_effects() -> Result<()> {
    let root = Context::new();
    let fiber = root.plugin_default(plugin_sync::<(), _>("alive", Inject::none(), |_, _| {
        Ok(PluginOutput::default())
    }));
    fiber.try_wait()?;
    assert!(root.fiber()?.restart().is_err());
    assert_eq!(root.fiber()?.state(), FiberState::Active);
    assert_eq!(fiber.state(), FiberState::Active);
    Ok(())
}

/// update() on an Active fiber pre-validates so a bad config cannot tear down
/// a running plugin; the validated result must be reused by the restart
/// instead of running the validator twice.
#[test]
fn update_validates_config_once() -> Result<()> {
    use cordis::utils::BoxFuture;
    use cordis::{Config, Plugin, PluginHandle};

    struct CountingValidator(Arc<AtomicUsize>);

    impl Plugin for CountingValidator {
        fn name(&self) -> &str {
            "counting"
        }

        fn validate_config(&self, config: Config) -> Result<Config> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(config)
        }

        fn apply(&self, _ctx: Context, _config: Config) -> BoxFuture<Result<PluginOutput>> {
            Box::pin(async { Ok(PluginOutput::default()) })
        }
    }

    let root = Context::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let fiber = root.plugin_default(PluginHandle::new(CountingValidator(calls.clone())));
    fiber.try_wait()?;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    fiber.update(())?;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    Ok(())
}

/// Regression: restart()/dispose() took a blocking lock on the transition
/// mutex that refresh() holds across user callbacks (status listeners,
/// disposers), so a listener restarting its own fiber deadlocked the thread.
#[test]
fn restart_from_status_listener_does_not_deadlock() -> Result<()> {
    let root = Context::new();
    let fiber = root.plugin_default(plugin_sync::<(), _>("reentrant", Inject::none(), |_, _| {
        Ok(PluginOutput::default())
    }));
    fiber.try_wait()?;
    let target_uid = fiber.uid();

    let fired = Arc::new(AtomicBool::new(false));
    let fired_in_listener = fired.clone();
    root.on("internal/status", move |event| {
        if fired_in_listener.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let Some(first) = event.args().first() else {
            return Ok(None);
        };
        let Ok(observed) = first.downcast::<cordis::Fiber>() else {
            return Ok(None);
        };
        if observed.uid() != target_uid {
            return Ok(None);
        }
        fired_in_listener.store(true, Ordering::SeqCst);
        // Reentrant restart on a fiber mid-transition: must fail fast,
        // not deadlock.
        let _ = observed.restart();
        Ok(None)
    })?;

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(fiber.restart());
    });
    rx.recv_timeout(std::time::Duration::from_secs(2))
        .expect("deadlock: reentrant restart from internal/status listener")?;
    assert!(fired.load(Ordering::SeqCst));
    Ok(())
}

/// A wrong-typed config update on a `plugin_sync` plugin must fail
/// validation and leave the running instance untouched. The type check
/// used to happen only inside `apply`, so the update tore the instance
/// down first and left the fiber `Failed`.
#[test]
fn update_with_wrong_config_type_fails_validation_not_the_plugin() -> Result<()> {
    let root = Context::new();
    let starts = Arc::new(AtomicUsize::new(0));
    let counter = starts.clone();
    let fiber = root.plugin(
        plugin_sync::<u32, _>("typed", Inject::none(), move |_, _| {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(PluginOutput::default())
        }),
        7_u32,
    );
    fiber.try_wait()?;
    assert_eq!(starts.load(Ordering::SeqCst), 1);

    let error = fiber.update("wrong type").unwrap_err();
    assert_eq!(error.code(), ErrorCode::TypeMismatch);
    assert_eq!(fiber.state(), FiberState::Active);
    assert_eq!(
        starts.load(Ordering::SeqCst),
        1,
        "the running instance is untouched"
    );
    fiber.dispose()?;
    Ok(())
}

/// Regression: `register_effect`'s liveness check and its push into the
/// effect list were not serialized against a concurrent `dispose`, so an
/// effect that landed after the disposal drain never ran its disposer.
/// Registration now re-checks liveness under the list lock and undoes the
/// push; hammer both sides and require every accepted effect to run.
#[test]
fn effect_registration_racing_disposal_never_leaks() {
    let mut leaks = 0;
    for _ in 0..4000 {
        let root = Context::new();
        let fiber = root.plugin_default(plugin_sync::<(), _>("raced", Inject::none(), |_, _| {
            Ok(PluginOutput::default())
        }));
        fiber.try_wait().unwrap();

        let accepted = Arc::new(AtomicUsize::new(0));
        let disposed = Arc::new(AtomicUsize::new(0));
        let registrar = {
            let (fiber, accepted, disposed) = (fiber.clone(), accepted.clone(), disposed.clone());
            std::thread::spawn(move || {
                for _ in 0..400 {
                    let disposed = disposed.clone();
                    match fiber.effect("raced", move || {
                        disposed.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    }) {
                        Ok(_) => {
                            accepted.fetch_add(1, Ordering::SeqCst);
                        }
                        Err(_) => return,
                    }
                }
            })
        };
        while accepted.load(Ordering::SeqCst) == 0 {
            std::thread::yield_now();
        }
        fiber.dispose().unwrap();
        registrar.join().unwrap();

        if disposed.load(Ordering::SeqCst) != accepted.load(Ordering::SeqCst) {
            leaks += 1;
        }
    }
    assert_eq!(
        leaks, 0,
        "some registered effects never ran their disposers"
    );
}
