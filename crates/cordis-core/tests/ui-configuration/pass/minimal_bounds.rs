use cordis_core::{ConfigurableService, Context, FiberHandle, InjectSpec, Plugin, PreparedChange, PreparedPlugin, Service};
use std::cell::Cell;
use std::fmt;
use std::future::{Future, Ready, ready};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, channel};
use std::task::{Context as TaskContext, Poll};
use parking_lot::Mutex;

#[derive(Debug)]
struct LocalError(Rc<()>);

impl fmt::Display for LocalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = &self.0;
        f.write_str("local error")
    }
}

impl std::error::Error for LocalError {}

struct LocalApplyFuture;

impl Future for LocalApplyFuture {
    type Output = Result<(), LocalError>;

    fn poll(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        Poll::Ready(Err(LocalError(Rc::new(()))))
    }
}

struct LocalConfig(Rc<()>);

struct MinimalPlugin(Cell<u8>);

impl Plugin for MinimalPlugin {
    type Config = LocalConfig;
    type Input = Receiver<()>;
    type PrepareError = LocalError;
    type ApplyError = LocalError;

    fn prepare(&self, config: Self::Config) -> Result<Self::Input, Self::PrepareError> {
        let _ = config.0;
        self.0.set(self.0.get() + 1);
        let (_tx, rx) = channel();
        Ok(rx)
    }

    fn apply(
        &self,
        _ctx: Context,
        _input: &Self::Input,
    ) -> impl Future<Output = Result<(), Self::ApplyError>> + Send {
        LocalApplyFuture
    }
}

struct MinimalService;

impl Service for MinimalService {
    const NAME: &'static str = "minimal";
}

impl ConfigurableService for MinimalService {
    type Config = LocalConfig;
    type Layer = Mutex<u8>;
    type Resolved = Receiver<()>;
    type PrepareError = LocalError;
    type ComposeError = LocalError;

    fn prepare_config(config: Self::Config) -> Result<Self::Layer, Self::PrepareError> {
        let _ = config.0;
        Ok(Mutex::new(1))
    }

    fn compose_config<'a>(
        _base: Option<&'a Self::Layer>,
        _layers: impl IntoIterator<Item = &'a Self::Layer>,
        _head: Option<&'a Self::Layer>,
    ) -> Result<Self::Resolved, Self::ComposeError> {
        let (_tx, rx) = channel();
        Ok(rx)
    }
}

fn main() {
    let plugin = MinimalPlugin(Cell::new(0));
    let prepared = plugin.prepare(LocalConfig(Rc::new(()))).unwrap();
    // sealing binds the exact (Plugin, Input) pair — no Sync on the
    // Plugin, no Send+Sync on the input value beyond the trait's own
    // `Input: Send` (a Receiver is Send but not Sync)
    let _sealed = PreparedPlugin::from_input(plugin, prepared);
    // the update/era candidate seals with the same minimal bounds
    let spare = MinimalPlugin(Cell::new(0));
    let candidate = spare.prepare(LocalConfig(Rc::new(()))).unwrap();
    let _change = PreparedChange::from_input::<MinimalPlugin>(candidate);
    let _layer = MinimalService::prepare_config(LocalConfig(Rc::new(()))).unwrap();
    let _: Ready<()> = ready(());
    let _ = InjectSpec::none();
}

// Spawn and update accept the sealed values without adding bounds of
// their own: the bodies only need to typecheck, never run.
#[allow(dead_code)]
async fn spawn_and_update(ctx: Context, fiber_handle: FiberHandle) {
    let plugin = MinimalPlugin(Cell::new(0));
    let prepared = plugin.prepare(LocalConfig(Rc::new(()))).unwrap();
    let _ = ctx.spawn(PreparedPlugin::from_input(plugin, prepared)).await;
    let spare = MinimalPlugin(Cell::new(0));
    let candidate = spare.prepare(LocalConfig(Rc::new(()))).unwrap();
    let _ = fiber_handle
        .update(PreparedChange::from_input::<MinimalPlugin>(candidate))
        .await;
}
