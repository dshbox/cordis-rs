use cordis_core::FiberHandle;
use std::any::Any;
fn raw(fiber_handle: &FiberHandle, value: Box<dyn Any + Send>) { let _ = fiber_handle.update(value); }
fn main() {}
