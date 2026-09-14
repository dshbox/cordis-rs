use cordis_core::{PreparedChange, Result};
use cordis_core::lifecycle::FiberHandle;

async fn check(fiber_handle: FiberHandle, change: PreparedChange) {
    let _: Result<FiberHandle> = fiber_handle.era_swap(change).await;
}

fn main() {}
