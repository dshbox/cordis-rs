use cordis_core::{FiberId, FiberHandle};

fn same_fiber(left: &FiberHandle, right: &FiberHandle) -> bool {
    left.id() == right.id()
}

fn correlate(id: &FiberId) -> (FiberId, String) {
    (id.clone(), format!("{id:?}"))
}

fn main() {
    let _ = same_fiber as fn(&FiberHandle, &FiberHandle) -> bool;
    let _ = correlate as fn(&FiberId) -> (FiberId, String);
}
