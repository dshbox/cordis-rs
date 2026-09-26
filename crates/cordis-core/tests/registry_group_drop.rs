//! Public API regression for the detached group-removal barrier.
use cordis_core::{Context, FiberState, Level, Plugin, PreparedPlugin, logger::BufferExporter};
use futures::FutureExt;
use std::{
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

struct Input {
    panic_on_drop: bool,
    cleanups: Arc<AtomicUsize>,
}

impl Drop for Input {
    fn drop(&mut self) {
        if self.panic_on_drop {
            panic!("final Input destructor");
        }
    }
}

struct Member;

impl Plugin for Member {
    type Config = Input;
    type Input = Input;
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, input: Input) -> Result<Input, Infallible> {
        Ok(input)
    }

    async fn apply(&self, ctx: Context, input: &Input) -> Result<(), Infallible> {
        let cleanups = input.cleanups.clone();
        ctx.effect_sync(move || {
            cleanups.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn group_removal_continues_after_a_members_final_input_drop_panics() {
    let root = Context::new();
    let reports = Arc::new(BufferExporter::new(8, Level::Warn).unwrap());
    let _reports_registration = root.add_exporter(reports.clone()).unwrap();
    let first_cleanups = Arc::new(AtomicUsize::new(0));
    let second_cleanups = Arc::new(AtomicUsize::new(0));
    let first = root
        .spawn(PreparedPlugin::from_input(
            Member,
            Input {
                panic_on_drop: true,
                cleanups: first_cleanups.clone(),
            },
        ))
        .await
        .unwrap();
    let second = root
        .spawn(PreparedPlugin::from_input(
            Member,
            Input {
                panic_on_drop: false,
                cleanups: second_cleanups.clone(),
            },
        ))
        .await
        .unwrap();
    drop(first);

    // The first Fiber's own terminal barrier completes before the frozen
    // group snapshot releases its last Arc. That Drop must not kill the
    // detached group owner before the second Fiber is cleaned up.
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        std::panic::AssertUnwindSafe(root.remove_plugins::<Member>()).catch_unwind(),
    )
    .await
    .expect("group removal must complete");
    assert!(
        result
            .expect("a member destructor must not unwind group removal")
            .is_ok()
    );
    assert_eq!(first_cleanups.load(Ordering::SeqCst), 1);
    assert_eq!(second_cleanups.load(Ordering::SeqCst), 1);
    assert_eq!(second.state(), FiberState::Disposed);
    assert!(reports.snapshot().iter().any(|record| {
        record.level() == Level::Warn
            && record
                .text()
                .contains("bulk removal member destruction panicked: final Input destructor")
    }));
    assert_eq!(root.runtime_snapshot().fibers().len(), 1); // root only

    // The detached allocation cannot be found on retry. The first call must
    // already have disposed every frozen member rather than rely on retry.
    root.remove_plugins::<Member>().await.unwrap();
    assert_eq!(second_cleanups.load(Ordering::SeqCst), 1);
    second.dispose().await.unwrap();
}
