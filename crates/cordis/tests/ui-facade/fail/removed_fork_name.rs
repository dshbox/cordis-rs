use cordis::Fork as RootFork;
use cordis::lifecycle::Fork as LifecycleFork;

fn main() {
    let _ = std::any::TypeId::of::<RootFork>();
    let _ = std::any::TypeId::of::<LifecycleFork>();
}
