use cordis_core::lifecycle::FiberHandle;

async fn check(fiber_handle: FiberHandle) {
    let _ = fiber_handle.era_swap(7u8).await;
}

fn main() {}
