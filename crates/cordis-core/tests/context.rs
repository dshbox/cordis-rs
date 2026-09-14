//! Legacy per-Fiber Scope regression tests. The v3 root and Service realm
//! contract lives in `realms.rs`.

use cordis_core::Context;
use std::sync::Arc;

// ADR 0003 overturn row 5: spawn derives a per-fiber scope node, so
// typed update-control layers registered by one fiber never intercept a
// sibling's update — while a layer on the shared parent scope (an
// ancestor) still sees both. This is the sibling-leak regression the
// overturn closes (v1 examples-api-tour #02).
#[tokio::test]
async fn typed_update_control_stays_scoped_to_each_fiber() {
    use cordis_core::event::around;
    use cordis_core::{Plugin, PreparedChange, PreparedPlugin};
    use std::borrow::Cow;
    use std::convert::Infallible;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Watcher {
        tag: &'static str,
        hits: Arc<AtomicU32>,
        veto: bool,
    }
    impl Plugin for Watcher {
        type Config = String;
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = Infallible;

        fn prepare(&self, _config: String) -> Result<(), Infallible> {
            Ok(())
        }

        fn name(&self) -> Cow<'static, str> {
            Cow::Borrowed(self.tag)
        }
        async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), Infallible> {
            let hits = self.hits.clone();
            let veto = self.veto;
            ctx.on_update::<Watcher, _>(
                around::<Watcher, _>(
                    move |_ctx, args, next: cordis_core::lifecycle::UpdateNext<Watcher>| {
                        let hits = hits.clone();
                        Box::pin(async move {
                            if veto {
                                // veto: do not call next — the swap never
                                // happens, and update() reports Ok
                                return Ok(());
                            }
                            hits.fetch_add(1, Ordering::SeqCst);
                            next.call(args).await?; // the restart runs
                            Ok::<(), cordis_core::event::InvocationFailure>(())
                        })
                    },
                ),
                Default::default(),
            )
            .unwrap();
            Ok(())
        }
    }

    let root = Context::new();
    let hits_parent = Arc::new(AtomicU32::new(0));

    // a parent-scope (ancestor) layer — registered FIRST so it sits
    // outermost in the onion and observes both fibers' updates all the
    // way through (a vetoing inner layer cuts only its own downstream)
    let parent_hits = hits_parent.clone();
    root.on_update::<Watcher, _>(
        around::<Watcher, _>(
            move |_ctx, args, next: cordis_core::lifecycle::UpdateNext<Watcher>| {
                let parent_hits = parent_hits.clone();
                Box::pin(async move {
                    parent_hits.fetch_add(1, Ordering::SeqCst);
                    next.call(args).await
                })
            },
        ),
        Default::default(),
    )
    .unwrap();

    let fiber_handle_a = root
        .spawn(PreparedPlugin::from_input(
            Watcher {
                tag: "watcher-a",
                hits: Arc::new(AtomicU32::new(0)),
                veto: true,
            },
            (),
        ))
        .await
        .unwrap();
    let fiber_handle_b = root
        .spawn(PreparedPlugin::from_input(
            Watcher {
                tag: "watcher-b",
                hits: Arc::new(AtomicU32::new(0)),
                veto: false,
            },
            (),
        ))
        .await
        .unwrap();

    // A's own layer vetoes A's update: no restart, Ok(())
    fiber_handle_a
        .update(PreparedChange::from_input::<Watcher>(()))
        .await
        .unwrap();
    assert_eq!(fiber_handle_a.state(), cordis_core::FiberState::Active);
    assert_eq!(hits_parent.load(Ordering::SeqCst), 1, "ancestor saw A");

    // B's update is untouched by A's veto layer — sibling Scopes are not
    // on each other's Scope ancestry
    fiber_handle_b
        .update(PreparedChange::from_input::<Watcher>(()))
        .await
        .unwrap();
    assert_eq!(fiber_handle_b.state(), cordis_core::FiberState::Active);
    assert_eq!(
        hits_parent.load(Ordering::SeqCst),
        2,
        "ancestor saw B too; A's veto never reached it"
    );
}

// Restart inherits the spawn scope: the fiber's typed update-control
// layer survives its own restart (re-applied onto the same scope node)
// and keeps vetoing.
#[tokio::test]
async fn restart_inherits_the_fibers_spawn_scope() {
    use cordis_core::event::around;
    use cordis_core::{Plugin, PreparedChange, PreparedPlugin};
    use std::borrow::Cow;
    use std::convert::Infallible;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct VetoingWatcher {
        hits: Arc<AtomicU32>,
    }
    impl Plugin for VetoingWatcher {
        type Config = String;
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = Infallible;

        fn prepare(&self, _config: String) -> Result<(), Infallible> {
            Ok(())
        }

        fn name(&self) -> Cow<'static, str> {
            Cow::Borrowed("vetoing-watcher")
        }
        async fn apply(&self, ctx: Context, _prepared: &()) -> Result<(), Infallible> {
            let hits = self.hits.clone();
            ctx.on_update::<VetoingWatcher, _>(
                around::<VetoingWatcher, _>(move |_ctx, _args, _next| {
                    let hits = hits.clone();
                    Box::pin(async move {
                        hits.fetch_add(1, Ordering::SeqCst);
                        // veto — restart must not happen through update()
                        Ok::<(), Infallible>(())
                    })
                }),
                Default::default(),
            )
            .unwrap();
            Ok(())
        }
    }

    let root = Context::new();
    let hits = Arc::new(AtomicU32::new(0));
    let fiber_handle = root
        .spawn(PreparedPlugin::from_input(
            VetoingWatcher { hits: hits.clone() },
            (),
        ))
        .await
        .unwrap();

    // the first apply's layer vetoes the first update
    fiber_handle
        .update(PreparedChange::from_input::<VetoingWatcher>(()))
        .await
        .unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    // a plain restart re-applies onto the same scope node; the fresh
    // layer vetoes again
    fiber_handle.restart().await.unwrap();
    assert_eq!(fiber_handle.state(), cordis_core::FiberState::Active);
    fiber_handle
        .update(PreparedChange::from_input::<VetoingWatcher>(()))
        .await
        .unwrap();
    assert_eq!(
        hits.load(Ordering::SeqCst),
        2,
        "the re-registered layer vetoed again"
    );
}
