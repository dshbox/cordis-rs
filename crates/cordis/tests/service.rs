use cordis::{
    Context, FiberState, Inject, PluginOutput, Result, Service, plugin_sync, service_async,
    service_sync,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Counter {
    value: AtomicUsize,
}

impl Service for Counter {
    const NAME: &'static str = "counter";
}

/// Upstream parity (service.spec 'apply functional plugin'): a synchronous
/// class-service plugin registers its service under `Service::NAME`, makes it
/// available to dependents once active, and removes it with the fiber.
#[test]
fn service_sync_registers_and_removes_typed_service() -> Result<()> {
    let root = Context::new();
    let plugin = service_sync::<Counter, (), _>("counter-provider", Inject::none(), |_ctx, _| {
        Ok(Counter {
            value: AtomicUsize::new(0),
        })
    });

    let fiber = root.plugin_default(plugin);
    fiber.try_wait()?;
    let service = root.require::<Counter>("counter")?;
    service.value.fetch_add(1, Ordering::SeqCst);
    assert_eq!(service.value.load(Ordering::SeqCst), 1);

    fiber.dispose()?;
    assert!(root.get::<Counter>("counter")?.is_none());
    Ok(())
}

/// Upstream parity (service.spec 'pending inject'): an asynchronous
/// class-service plugin stays Pending until its dependencies arrive, then
/// constructs and provides the service exactly once.
#[test]
fn service_async_defers_until_dependencies_arrive() -> Result<()> {
    let root = Context::new();
    let built = Arc::new(AtomicUsize::new(0));
    let plugin = service_async::<Counter, (), _, _>("counter-async", Inject::new(["database"]), {
        let built = built.clone();
        move |_ctx, _| {
            let built = built.clone();
            async move {
                built.fetch_add(1, Ordering::SeqCst);
                Ok(Counter {
                    value: AtomicUsize::new(0),
                })
            }
        }
    });

    let fiber = root.plugin_default(plugin);
    assert_eq!(fiber.state(), FiberState::Pending);
    assert_eq!(built.load(Ordering::SeqCst), 0);

    let _database = root.provide("database", 7_u32)?;
    fiber.try_wait()?;
    assert_eq!(built.load(Ordering::SeqCst), 1);
    assert_eq!(
        root.require::<Counter>("counter")?
            .value
            .load(Ordering::SeqCst),
        0
    );
    Ok(())
}

struct GateService {
    available: AtomicBool,
}

impl Service for GateService {
    const NAME: &'static str = "gate";

    fn is_available(&self) -> bool {
        self.available.load(Ordering::SeqCst)
    }
}

/// Upstream parity (service.spec availability contract): a service whose
/// `is_available()` predicate is false keeps dependents Pending until the
/// predicate flips and the change is notified.
#[test]
fn availability_predicate_gates_dependents() -> Result<()> {
    let root = Context::new();
    let service = Arc::new(GateService {
        available: AtomicBool::new(false),
    });
    let _handle = root.provide_service_arc(service.clone())?;

    let fiber = root.plugin_default(plugin_sync::<(), _>("consumer", Inject::new(["gate"]), {
        move |ctx, _| {
            ctx.require::<GateService>("gate")?;
            Ok(PluginOutput::none())
        }
    }));
    assert_eq!(fiber.state(), FiberState::Pending);

    service.available.store(true, Ordering::SeqCst);
    root.notify(["gate"]);
    fiber.try_wait()?;
    assert_eq!(fiber.state(), FiberState::Active);
    Ok(())
}

/// `provide_service_arc` stores the given `Arc` without double-wrapping, so
/// `require` returns the very same allocation.
#[test]
fn provide_service_arc_keeps_identity() -> Result<()> {
    let root = Context::new();
    let service = Arc::new(Counter {
        value: AtomicUsize::new(0),
    });
    let _handle = root.provide_service_arc(service.clone())?;

    let fetched = root.require::<Counter>("counter")?;
    assert!(Arc::ptr_eq(&service, &fetched));
    Ok(())
}

/// Upstream parity (invoke.spec intercept merge): `resolve_service_config`
/// merges intercept entries root-first with the caller-supplied operation, so
/// a pick-last merge reproduces the upstream leaf/nearest-wins precedence.
#[test]
fn resolve_service_config_merges_root_to_leaf() -> Result<()> {
    let root = Context::new();
    let child = root.intercept("svc", 1_u32).intercept("svc", 2_u32);

    // No intercepts above the root context: the base passes through.
    let merged = root.resolve_service_config("svc", 0_u32, |acc, next| acc + *next)?;
    assert_eq!(merged, 0);

    // Root-first accumulation sees both layers in declaration order.
    let merged = child.resolve_service_config("svc", 0_u32, |acc, next| acc + *next)?;
    assert_eq!(merged, 3);

    // A pick-last merge keeps the nearest (leaf) entry.
    let merged = child.resolve_service_config("svc", 0_u32, |_, next| *next)?;
    assert_eq!(merged, 2);
    Ok(())
}
