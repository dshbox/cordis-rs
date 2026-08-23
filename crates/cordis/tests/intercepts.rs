//! The five upstream intercept meta-events: `internal/get`, `internal/set`,
//! `internal/config`, `internal/update`, and `internal/listener`.

use cordis::utils::block_on;
use cordis::{Accessor, Context, ErrorCode, FiberState, PluginOutput, Result, Value, plugin_sync};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[test]
fn internal_get_rewrites_strict_reads_and_skips_accessors_and_relaxed() -> Result<()> {
    let root = Context::new();
    root.provide("svc", 42_u32)?;
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));

    let seen_in_listener = seen.clone();
    root.on("internal/get", move |event| {
        // The interception event carries the operating context as its target
        // and the service name as its argument.
        assert!(event.target().is_some());
        let name = event.arg::<String>(0)?.unwrap();
        seen_in_listener.lock().unwrap().push(name.to_string());
        let next = block_on(event.call_next())?;
        if name.as_str() == "svc" {
            Ok(Some(Value::new(7_u32)))
        } else {
            Ok(next)
        }
    })?;

    // Strict reads are intercepted and rewritten.
    assert_eq!(*root.require::<u32>("svc")?, 7);
    assert_eq!(root.get::<u32>("svc")?.unwrap().as_ref(), &7_u32);

    // Relaxed reads bypass the interception.
    assert_eq!(*root.get_relaxed::<u32>("svc")?.unwrap(), 42);

    // Accessor reads bypass the interception (upstream proxy parity).
    let _accessor = root.accessor(
        "computed",
        Accessor::read_only(|_| Ok(Some(Value::new(1_u32)))),
    )?;
    assert_eq!(*root.require::<u32>("computed")?, 1);

    assert_eq!(
        *seen.lock().unwrap(),
        vec!["svc".to_owned(), "svc".to_owned()]
    );
    Ok(())
}

#[test]
fn internal_get_failure_fails_the_read() -> Result<()> {
    let root = Context::new();
    root.provide("svc", 1_u32)?;
    root.on("internal/get", |event| {
        let name = event.arg::<String>(0)?.unwrap();
        if name.as_str() == "svc" {
            Err(cordis::CordisError::with_message(
                ErrorCode::Event,
                "get intercepted",
            ))
        } else {
            block_on(event.call_next())
        }
    })?;
    let error = root.require::<u32>("svc").unwrap_err();
    assert!(error.to_string().contains("get intercepted"));
    Ok(())
}

#[test]
fn internal_set_observes_and_can_veto_writes() -> Result<()> {
    let root = Context::new();
    root.provide("svc", 1_u32)?;
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));

    let seen_in_listener = seen.clone();
    root.on("internal/set", move |event| {
        let name = event.arg::<String>(0)?.unwrap();
        let value = *event.arg::<u32>(1)?.unwrap();
        seen_in_listener
            .lock()
            .unwrap()
            .push(format!("{}={value}", name.as_str()));
        block_on(event.call_next())
    })?;

    root.set("svc", 2_u32)?;
    assert_eq!(*root.require::<u32>("svc")?, 2);

    // A later listener vetoes the write of 3 by not continuing the chain.
    root.on("internal/set", |event| {
        let value = *event.arg::<u32>(1)?.unwrap();
        if value == 3 {
            Ok(None)
        } else {
            block_on(event.call_next())
        }
    })?;
    root.set("svc", 3_u32)?;
    assert_eq!(
        *root.require::<u32>("svc")?,
        2,
        "vetoed write must not land"
    );

    assert_eq!(
        *seen.lock().unwrap(),
        vec!["svc=2".to_owned(), "svc=3".to_owned()]
    );
    Ok(())
}

#[test]
fn internal_set_failure_fails_the_set() -> Result<()> {
    let root = Context::new();
    root.provide("svc", 1_u32)?;
    root.on("internal/set", |event| {
        let name = event.arg::<String>(0)?.unwrap();
        if name.as_str() == "svc" {
            Err(cordis::CordisError::with_message(
                ErrorCode::Event,
                "set rejected",
            ))
        } else {
            block_on(event.call_next())
        }
    })?;
    let error = root.set("svc", 2_u32).unwrap_err();
    assert!(error.to_string().contains("set rejected"));
    assert_eq!(*root.require::<u32>("svc")?, 1, "failed set must not land");
    Ok(())
}

#[test]
fn internal_config_rewrites_config_on_start_and_update() -> Result<()> {
    let root = Context::new();
    let intercepted = Arc::new(Mutex::new(Vec::<u32>::new()));
    let applied = Arc::new(Mutex::new(Vec::<u32>::new()));

    let intercepted_in_listener = intercepted.clone();
    root.on("internal/config", move |event| {
        let config = *event.arg::<u32>(0)?.unwrap();
        intercepted_in_listener.lock().unwrap().push(config);
        Ok(Some(Value::new(config * 2)))
    })?;

    let applied_in_plugin = applied.clone();
    let fiber = root.plugin(
        plugin_sync::<u32, _>("cfg", Default::default(), move |_, config| {
            applied_in_plugin.lock().unwrap().push(*config);
            Ok(PluginOutput::none())
        }),
        5_u32,
    );
    fiber.try_wait()?;
    assert_eq!(*applied.lock().unwrap(), vec![10], "startup config doubled");
    assert_eq!(*fiber.config().downcast::<u32>()?, 10);

    fiber.update(21_u32)?;
    fiber.try_wait()?;
    assert_eq!(
        *applied.lock().unwrap(),
        vec![10, 42],
        "update config doubled"
    );
    assert_eq!(*fiber.config().downcast::<u32>()?, 42);
    assert_eq!(*intercepted.lock().unwrap(), vec![5, 21]);
    Ok(())
}

#[test]
fn internal_config_failure_fails_activation() -> Result<()> {
    let root = Context::new();
    root.on("internal/config", |_| {
        Err(cordis::CordisError::with_message(
            ErrorCode::Event,
            "config rejected",
        ))
    })?;
    let fiber = root.plugin(
        plugin_sync::<u32, _>("cfg", Default::default(), |_, _| Ok(PluginOutput::none())),
        1_u32,
    );
    assert_eq!(fiber.state(), FiberState::Failed);
    let error = fiber.error().unwrap();
    assert!(error.to_string().contains("config rejected"));
    Ok(())
}

#[test]
fn internal_update_can_veto_and_then_allow_restarts() -> Result<()> {
    let root = Context::new();
    let applies = Arc::new(AtomicUsize::new(0));
    let applies_in_plugin = applies.clone();
    let fiber = root.plugin(
        plugin_sync::<u32, _>("up", Default::default(), move |_, _| {
            applies_in_plugin.fetch_add(1, Ordering::SeqCst);
            Ok(PluginOutput::none())
        }),
        1_u32,
    );
    fiber.try_wait()?;
    assert_eq!(applies.load(Ordering::SeqCst), 1);

    let veto = Arc::new(AtomicBool::new(true));
    let veto_in_listener = veto.clone();
    root.on("internal/update", move |event| {
        if veto_in_listener.load(Ordering::SeqCst) {
            Ok(None) // veto: do not continue to the restart
        } else {
            block_on(event.call_next())
        }
    })?;

    // Vetoed update: the config is stored but the fiber is not restarted.
    fiber.update(2_u32)?;
    assert_eq!(applies.load(Ordering::SeqCst), 1, "vetoed update restarted");
    assert_eq!(*fiber.config().downcast::<u32>()?, 1);
    assert_eq!(fiber.state(), FiberState::Active);

    // Lifting the veto lets the next update restart with its config.
    veto.store(false, Ordering::SeqCst);
    fiber.update(3_u32)?;
    fiber.try_wait()?;
    assert_eq!(applies.load(Ordering::SeqCst), 2);
    assert_eq!(*fiber.config().downcast::<u32>()?, 3);
    Ok(())
}

#[test]
fn internal_update_failure_fails_the_update() -> Result<()> {
    let root = Context::new();
    let fiber = root.plugin(
        plugin_sync::<u32, _>("up", Default::default(), |_, _| Ok(PluginOutput::none())),
        1_u32,
    );
    fiber.try_wait()?;
    root.on("internal/update", |_| {
        Err(cordis::CordisError::with_message(
            ErrorCode::Event,
            "update vetoed",
        ))
    })?;
    let error = fiber.update(2_u32).unwrap_err();
    assert!(error.to_string().contains("update vetoed"));
    assert_eq!(fiber.state(), FiberState::Active, "failed update restarted");
    Ok(())
}

#[test]
fn internal_listener_bail_cancels_registration() -> Result<()> {
    let root = Context::new();
    root.on("internal/listener", |event| {
        let name = event.arg::<String>(0)?.unwrap();
        if name.as_str() == "blocked" {
            Ok(Some(Value::new(()))) // take over: cancel the registration
        } else {
            Ok(None)
        }
    })?;

    let fired = Arc::new(AtomicUsize::new(0));
    let fired_in_listener = fired.clone();
    let handle = root.on("blocked", move |_| {
        fired_in_listener.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    })?;
    root.emit("blocked", [])?;
    assert_eq!(
        fired.load(Ordering::SeqCst),
        0,
        "registration was cancelled"
    );
    handle.dispose()?; // the inert handle disposes cleanly and is idempotent

    // Unintercepted names register and fire normally.
    let allowed = Arc::new(AtomicUsize::new(0));
    let allowed_in_listener = allowed.clone();
    root.on("allowed", move |_| {
        allowed_in_listener.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    })?;
    root.emit("allowed", [])?;
    assert_eq!(allowed.load(Ordering::SeqCst), 1);
    Ok(())
}
