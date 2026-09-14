use cordis_core::{Context, FiberId, FiberHandle};

fn numeric_conversion(id: FiberId) {
    let _: u64 = id.into();
}

fn obsolete_uid(fiber_handle: &FiberHandle) {
    let _ = fiber_handle.uid();
}

fn identity_is_not_a_lookup_capability(ctx: &Context, id: FiberId) {
    let _ = ctx.lookup_fiber(id);
}

fn main() {}
