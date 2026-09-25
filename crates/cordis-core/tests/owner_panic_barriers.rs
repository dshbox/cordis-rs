//! Real Tokio probes for user-owned destruction at committed lifecycle boundaries.
use cordis_core::{Context, FiberState, Plugin, PreparedChange, PreparedPlugin};
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

struct DropInput {
    panic_on_drop: bool,
    drops: Arc<AtomicUsize>,
}

impl Drop for DropInput {
    fn drop(&mut self) {
        if self.panic_on_drop {
            self.drops.fetch_add(1, Ordering::SeqCst);
            panic!("superseded input Drop panicked");
        }
    }
}

struct InputOwner {
    applies: Arc<AtomicUsize>,
}
impl Plugin for InputOwner {
    type Config = ();
    type Input = DropInput;
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, (): ()) -> Result<DropInput, Infallible> {
        unreachable!()
    }
    async fn apply(&self, _: Context, _: &DropInput) -> Result<(), Infallible> {
        self.applies.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn panicking_superseded_input_after_update_commit_does_not_strand_ready() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let ctx = Context::new();
    let drops = Arc::new(AtomicUsize::new(0));
    let applies = Arc::new(AtomicUsize::new(0));
    let fiber = runtime
        .block_on(ctx.spawn(PreparedPlugin::from_input(
            InputOwner {
                applies: applies.clone(),
            },
            DropInput {
                panic_on_drop: true,
                drops: drops.clone(),
            },
        )))
        .unwrap();
    let updating = fiber.clone();
    let change = PreparedChange::from_input::<InputOwner>(DropInput {
        panic_on_drop: false,
        drops: drops.clone(),
    });
    let join = runtime
        .block_on(async move { tokio::spawn(async move { updating.update(change).await }).await });
    assert!(
        join.unwrap_err().is_panic(),
        "user Drop panic stays observable"
    );
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    let observer = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let ready = observer
        .block_on(async { tokio::time::timeout(Duration::from_secs(2), fiber.ready()).await });
    assert!(
        matches!(ready, Ok(Ok(FiberState::Active))),
        "separate runtime ready: {ready:?}"
    );
    assert_eq!(applies.load(Ordering::SeqCst), 2, "new input was applied");
}

#[test]
fn panicking_superseded_input_during_era_swap_leaves_source_terminal() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let ctx = Context::new();
    let drops = Arc::new(AtomicUsize::new(0));
    let applies = Arc::new(AtomicUsize::new(0));
    let source = runtime
        .block_on(ctx.spawn(PreparedPlugin::from_input(
            InputOwner {
                applies: applies.clone(),
            },
            DropInput {
                panic_on_drop: true,
                drops: drops.clone(),
            },
        )))
        .unwrap();
    let swapping = source.clone();
    let change = PreparedChange::from_input::<InputOwner>(DropInput {
        panic_on_drop: false,
        drops: drops.clone(),
    });
    let join = runtime.block_on(async move {
        tokio::spawn(async move { swapping.era_swap(change).await }).await
    });
    assert!(
        join.unwrap_err().is_panic(),
        "owner loss is observable as a panic"
    );
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    let observer = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let ready = observer
        .block_on(async { tokio::time::timeout(Duration::from_secs(2), source.ready()).await });
    assert!(
        matches!(ready, Ok(Ok(FiberState::Disposed))),
        "source barrier: {ready:?}"
    );
    assert_eq!(applies.load(Ordering::SeqCst), 1, "no successor apply ran");
}

struct BlockingInput(Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>);
impl Drop for BlockingInput {
    fn drop(&mut self) {
        if let Some((entered, release)) = self.0.take() {
            entered.send(()).unwrap();
            release.recv().unwrap();
        }
    }
}
struct BlockingOwner(Arc<AtomicUsize>);
impl Plugin for BlockingOwner {
    type Config = ();
    type Input = BlockingInput;
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, _: ()) -> Result<BlockingInput, Infallible> {
        Ok(BlockingInput(None))
    }
    fn apply(
        &self,
        _: Context,
        _: &BlockingInput,
    ) -> impl std::future::Future<Output = Result<(), Infallible>> + Send {
        self.0.fetch_add(1, Ordering::SeqCst);
        std::future::ready(Ok(()))
    }
}

#[test]
fn new_update_apply_waits_until_superseded_input_drop_finishes() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let root = Context::new();
    let applies = Arc::new(AtomicUsize::new(0));
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let fiber = runtime
        .block_on(root.spawn(PreparedPlugin::from_input(
            BlockingOwner(applies.clone()),
            BlockingInput(Some((entered_tx, release_rx))),
        )))
        .unwrap();
    let updating = fiber.clone();
    let update = runtime.handle().spawn(async move {
        updating
            .update(PreparedChange::from_input::<BlockingOwner>(BlockingInput(
                None,
            )))
            .await
    });
    entered_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("old input Drop started");
    assert_eq!(
        applies.load(Ordering::SeqCst),
        1,
        "new apply cannot precede old Drop"
    );
    release_tx.send(()).unwrap();
    runtime.block_on(update).unwrap().unwrap();
    assert_eq!(applies.load(Ordering::SeqCst), 2);
}
