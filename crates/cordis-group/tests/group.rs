//! Group fiber nesting: children started under the group's context die with
//! it — the model the loader relies on.

use cordis::{Context, FiberState, Inject, PluginOutput, plugin_sync};
use cordis_group::{GROUP_NAME, Group};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn group_plugin_starts_active_under_its_registered_name() {
    let root = Context::new();
    let fiber = root.plugin_default(Group::handle());
    fiber.try_wait().unwrap();
    assert_eq!(fiber.name(), GROUP_NAME);
    assert_eq!(fiber.state(), FiberState::Active);
}

#[test]
fn children_started_under_the_group_context_cascade_on_dispose() {
    let root = Context::new();
    let group = root.plugin_default(Group::handle());
    group.try_wait().unwrap();

    let stopped = Arc::new(AtomicUsize::new(0));
    let group_ctx = group.context().expect("group context");
    let child = group_ctx.plugin_default(plugin_sync::<(), _>("child", Inject::default(), {
        let stopped = stopped.clone();
        move |ctx, _| {
            let stopped = stopped.clone();
            ctx.fiber()?.effect("stop", move || {
                stopped.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })?;
            Ok(PluginOutput::none())
        }
    }));
    child.try_wait().unwrap();

    group.dispose().unwrap();
    assert_eq!(child.state(), FiberState::Disposed);
    assert_eq!(stopped.load(Ordering::SeqCst), 1);
}
