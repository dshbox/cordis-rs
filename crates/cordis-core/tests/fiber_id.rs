//! Public identity evidence: IDs correlate one Fiber and grant no control.

use cordis_core::{Context, FiberId, Plugin, PreparedPlugin};
use std::convert::Infallible;
use std::future::{Future, ready};

struct Noop;

impl Plugin for Noop {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn apply(
        &self,
        _ctx: Context,
        _prepared: &(),
    ) -> impl Future<Output = Result<(), Infallible>> + Send {
        ready(Ok(()))
    }
}

#[tokio::test]
async fn cloned_fiber_handles_share_an_opaque_id_and_runtimes_do_not() {
    let first_context = Context::new();
    let first = first_context
        .spawn(PreparedPlugin::from_input(Noop, ()))
        .await
        .unwrap();
    let clone = first.clone();
    let first_id: FiberId = first.id();

    assert_eq!(first_id, clone.id());
    assert_eq!(format!("{first_id:?}"), "FiberId(..)");

    let sibling = first_context
        .spawn(PreparedPlugin::from_input(Noop, ()))
        .await
        .unwrap();
    assert_ne!(first.id(), sibling.id());

    let second_context = Context::new();
    let second = second_context
        .spawn(PreparedPlugin::from_input(Noop, ()))
        .await
        .unwrap();
    assert_ne!(first.id(), second.id());

    first.dispose().await.unwrap();
    assert_eq!(clone.id(), first_id);
    sibling.dispose().await.unwrap();
    second.dispose().await.unwrap();
}
