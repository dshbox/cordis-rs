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
    assert_eq!(root.fiber()?.get_effects().len(), 3);
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
