use cordis_core::FiberHandle;

async fn old_restart_channel(fiber_handle: &FiberHandle) {
    let _: cordis_core::Result<()> = fiber_handle.restart().await;
}

fn main() {}
