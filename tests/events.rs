use cordis::utils::block_on;
use cordis::{Context, EventOptions, Result, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[test]
fn on_once_prepend_and_disposal() -> Result<()> {
    let root = Context::new();
    let order = Arc::new(Mutex::new(Vec::new()));

    let first = order.clone();
    let handle = root.on("ready", move |_| {
        first.lock().unwrap().push(1);
        Ok(None)
    })?;
    let prepended = order.clone();
    root.on_with(
        "ready",
        move |_| {
            prepended.lock().unwrap().push(0);
            Ok(None)
        },
        EventOptions {
            prepend: true,
            global: false,
        },
    )?;
    let once = order.clone();
    root.once("ready", move |_| {
        once.lock().unwrap().push(2);
        Ok(None)
    })?;

    root.emit("ready", [])?;
    root.emit("ready", [])?;
    handle.dispose()?;
    root.emit("ready", [])?;
    assert_eq!(*order.lock().unwrap(), vec![0, 1, 2, 0, 1, 0]);
    Ok(())
}

#[test]
fn bail_serial_parallel_and_waterfall() -> Result<()> {
    let root = Context::new();
    root.on("query", |_| Ok(None))?;
    root.on("query", |_| Ok(Some(Value::new(42_u32))))?;
    root.on("query", |_| panic!("must have bailed"))?;
    let value = root
        .events()
        .bail("query", [])?
        .unwrap()
        .downcast::<u32>()?;
    assert_eq!(*value, 42);

    let calls = Arc::new(AtomicUsize::new(0));
    for _ in 0..3 {
        let calls = calls.clone();
        root.on_async("parallel", move |_| {
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            }
        })?;
    }
    block_on(root.events().parallel("parallel", []))?;
    assert_eq!(calls.load(Ordering::SeqCst), 3);

    root.on_async("sum", |event| async move {
        let value = *event.arg::<u32>(0)?.unwrap();
        let next = event.next().await?.unwrap().downcast::<u32>()?;
        Ok(Some(Value::new(value + *next)))
    })?;
    root.on_async("sum", |event| async move {
        let value = *event.arg::<u32>(0)?.unwrap();
        let next = event.next().await?.unwrap().downcast::<u32>()?;
        Ok(Some(Value::new(value + *next)))
    })?;
    let result = block_on(
        root.events()
            .waterfall_async("sum", [Value::new(1_u32)], || async {
                Ok(Some(Value::new(2_u32)))
            }),
    )?
    .unwrap()
    .downcast::<u32>()?;
    assert_eq!(*result, 4);
    Ok(())
}

#[test]
fn dispatch_filter_and_global_listener() -> Result<()> {
    let root = Context::new();
    let local = Arc::new(AtomicUsize::new(0));
    let global = Arc::new(AtomicUsize::new(0));
    let listener_context = root.extend("enabled", true);

    let local_count = local.clone();
    listener_context.on("filtered", move |_| {
        local_count.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    })?;
    let global_count = global.clone();
    listener_context.on_with(
        "filtered",
        move |_| {
            global_count.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        },
        EventOptions {
            prepend: false,
            global: true,
        },
    )?;

    let rejected = root.with_filter(|ctx| {
        ctx.metadata::<bool>("enabled")
            .ok()
            .flatten()
            .map(|value| !*value)
            .unwrap_or(false)
    });
    root.events().emit_from(rejected, "filtered", [])?;
    assert_eq!(local.load(Ordering::SeqCst), 0);
    assert_eq!(global.load(Ordering::SeqCst), 1);
    Ok(())
}

/// Upstream parity: the internal/dispatch meta-event is gated on listener
/// presence; with a listener registered it carries the dispatch details.
#[test]
fn internal_dispatch_meta_event() -> Result<()> {
    let root = Context::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_in_listener = seen.clone();
    root.on("internal/dispatch", move |event| {
        seen_in_listener
            .lock()
            .unwrap()
            .push(event.arg::<String>(1)?);
        Ok(None)
    })?;
    root.emit("user-event", [])?;
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].as_ref().map(|name| name.as_str()),
        Some("user-event")
    );
    Ok(())
}

/// Regression: dispatch invoked user ContextFilters while holding the events
/// state lock, so a filter re-entering any events API deadlocked the thread.
#[test]
fn filter_may_reenter_events_api() -> Result<()> {
    let root = Context::new();
    root.on("reentry", |_| Ok(None))?;

    let filtered = root.with_filter(|owner| {
        // Re-entering the events API from a filter must not deadlock.
        let _ = owner.events().listener_count("reentry");
        true
    });

    let events = root.events();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(events.emit_from(filtered, "reentry", []));
    });
    rx.recv_timeout(std::time::Duration::from_secs(2))
        .expect("deadlock: ContextFilter re-entering the events API")?;
    Ok(())
}

/// Upstream parity: a once listener is removed on invocation, so an earlier
/// bail must not unregister listeners that never ran.
#[test]
fn once_survives_earlier_bail() -> Result<()> {
    let root = Context::new();
    let bailing = root.on("stop", |_| Ok(Some(Value::new(1_u32))))?;
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_in_listener = hits.clone();
    root.once("stop", move |_| {
        hits_in_listener.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    })?;

    // The earlier listener bails; the once listener never runs and stays
    // registered.
    root.events().bail("stop", [])?;
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    assert_eq!(root.events().listener_count("stop"), 2);

    // With the bailing listener gone, the once listener fires exactly once.
    bailing.dispose()?;
    root.events().bail("stop", [])?;
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(root.events().listener_count("stop"), 0);
    Ok(())
}
