use cordis_core::FiberHandle;

async fn old_dispose_channel(fiber_handle: &FiberHandle) {
    let _: cordis_core::Result<()> = fiber_handle.dispose().await;
}

fn main() {}
