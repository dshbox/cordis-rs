use cordis::{Context, FiberState, Inject, PluginOutput, Result, plugin_sync};
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
    fiber.wait()?;
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
    assert!(fiber.wait().is_err());
    root.emit("probe", [])?;
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    fiber.update(true)?;
    assert_eq!(fiber.state(), FiberState::Active);
    root.emit("probe", [])?;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
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
    parent.wait()?;
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
        fiber.wait()?;
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
    fiber.wait()?;
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
    fiber.wait()?;
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
