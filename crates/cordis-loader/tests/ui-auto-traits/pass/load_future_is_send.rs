//! `LoadPlan::load` returns a `Send` future exactly when its resolver is
//! `Sync`, as the normative interface promises (§Auto traits). The signature
//! is not tightened: a resolver that is not `Sync` can still load, and its
//! future is only not promised `Send`.

use std::cell::Cell;

use cordis_core::{Context, PreparedPlugin};
use cordis_loader::{LoadPlan, PluginResolver, resolver::PluginRequest};

fn assert_send<T: Send>(_: &T) {}

fn load_is_send<R: PluginResolver + Sync + ?Sized>(plan: &LoadPlan, ctx: &Context, resolver: &R) {
    assert_send(&plan.load(ctx, resolver));
}

/// A resolver with interior mutability that is `Send` but not `Sync`.
struct Counting {
    calls: Cell<usize>,
}

impl PluginResolver for Counting {
    type Error = std::io::Error;

    fn resolve(
        &self,
        _request: PluginRequest<'_>,
    ) -> Result<Option<PreparedPlugin>, Self::Error> {
        self.calls.set(self.calls.get() + 1);
        Ok(None)
    }
}

async fn non_sync_resolver_still_loads(plan: &LoadPlan, ctx: &Context) -> usize {
    let resolver = Counting { calls: Cell::new(0) };
    let outcome = plan.load(ctx, &resolver).await;
    let _ = outcome.is_ok();
    resolver.calls.get()
}

fn main() {
    let _ = load_is_send::<dyn PluginResolver<Error = std::io::Error> + Sync>;
    let _ = non_sync_resolver_still_loads;
}
