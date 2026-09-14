use cordis_core::Fork as RootFork;
use cordis_core::lifecycle::Fork as LifecycleFork;

fn main() {
    let _ = std::any::TypeId::of::<RootFork>();
    let _ = std::any::TypeId::of::<LifecycleFork>();
}
