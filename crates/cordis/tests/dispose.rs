use cordis::{
    Context, CordisError, EffectMeta, ErrorCode, Inject, LogKind, PluginOutput, Result, plugin_sync,
};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Upstream parity (dispose.spec 'dispose by plugin' + fiber.spec 'dispose
/// error'): a disposer that fails must not break teardown — the fiber still
/// disposes, every other disposer still runs in reverse registration order,
/// and the failure is routed to the logger instead of propagating.
#[test]
fn failing_disposer_is_isolated_and_teardown_completes() -> Result<()> {
    let root = Context::new();
    let order = Arc::new(Mutex::new(Vec::new()));
    let recorded = order.clone();
    let plugin = plugin_sync::<(), _>("failing", Inject::none(), move |ctx, _| {
        ctx.effect("boom", || {
            Err(CordisError::with_message(ErrorCode::Other, "boom"))
        })?;
        for value in 1..=3 {
            let recorded = recorded.clone();
            ctx.effect_infallible(format!("effect {value}"), move || {
                recorded.lock().unwrap().push(value);
            })?;
        }
        Ok(PluginOutput::none())
    });

    let fiber = root.plugin_default(plugin);
    fiber.try_wait()?;
    fiber.dispose()?;

    // The failing disposer registered first, so LIFO teardown runs 3, 2, 1,
    // then the failing one — which must not prevent the others from running.
    assert_eq!(*order.lock().unwrap(), vec![3, 2, 1]);

    // The failure reached the logger rather than being silently swallowed.
    let buffer = root.logger_service().buffer();
    assert!(buffer.iter().any(|message| message.kind == LogKind::Error));
    Ok(())
}

/// Upstream parity (dispose.spec 'yield dispose'): adopted effects appear as
/// a nested diagnostic tree on the owning fiber, are disposed together with
/// their parent, and double disposal is a no-op.
#[test]
fn adopted_effects_form_a_diagnostic_tree() -> Result<()> {
    let root = Context::new();
    let inner_calls = Arc::new(AtomicUsize::new(0));

    let outer = root.effect_infallible("outer", || {})?;
    let inner = root.effect_infallible("inner", {
        let inner_calls = inner_calls.clone();
        move || {
            inner_calls.fetch_add(1, Ordering::SeqCst);
        }
    })?;
    assert_eq!(root.fiber()?.effects().len(), 2);

    outer.adopt(inner)?;
    assert_eq!(
        root.fiber()?.effects(),
        vec![EffectMeta {
            label: "outer".to_owned(),
            children: vec![EffectMeta::new("inner")],
        }]
    );

    outer.dispose()?;
    assert_eq!(inner_calls.load(Ordering::SeqCst), 1);
    outer.dispose()?;
    assert_eq!(inner_calls.load(Ordering::SeqCst), 1);
    Ok(())
}
