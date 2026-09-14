use cordis_core::{FiberState, FiberHandle};

async fn old_ready_channel(fiber_handle: &FiberHandle) {
    let _: cordis_core::Result<FiberState> = fiber_handle.ready().await;
}

fn main() {}
