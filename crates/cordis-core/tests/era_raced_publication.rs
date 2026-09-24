//! Production Tokio regression for an era source Service edge published while the source slot is busy.

use cordis_core::{
    Context, FiberState, InjectSpec, Plugin, PreparedChange, PreparedPlugin, Service,
};
use std::{
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::sync::Notify;

struct S;
impl Service for S {
    const NAME: &'static str = "era-raced-publication";
}

struct Signals {
    applies: AtomicUsize,
    source_entered: Notify,
    source_release: Notify,
    dependent_entered: Notify,
    dependent_release: Notify,
}
struct Source(Arc<Signals>);
impl Plugin for Source {
    type Config = ();
    type Input = u8;
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, (): ()) -> Result<u8, Infallible> {
        Ok(1)
    }
    fn apply(
        &self,
        ctx: Context,
        input: &u8,
    ) -> impl std::future::Future<Output = Result<(), Infallible>> + Send {
        let signals = self.0.clone();
        let input = *input;
        async move {
            let attempt = signals.applies.fetch_add(1, Ordering::SeqCst) + 1;
            if attempt == 2 && input == 1 {
                signals.source_entered.notify_one();
                signals.source_release.notified().await;
                let _publication = ctx.provide(Arc::new(S)).unwrap();
            }
            Ok(())
        }
    }
}
struct Dependent(Arc<Signals>);
impl Plugin for Dependent {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;
    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    fn inject(&self) -> InjectSpec {
        InjectSpec::none().require(S::NAME)
    }
    fn apply(
        &self,
        _: Context,
        _: &(),
    ) -> impl std::future::Future<Output = Result<(), Infallible>> + Send {
        let signals = self.0.clone();
        async move {
            signals.dependent_entered.notify_one();
            signals.dependent_release.notified().await;
            Ok(())
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn era_swap_waits_for_service_added_while_waiting_for_source_slot() {
    let signals = Arc::new(Signals {
        applies: AtomicUsize::new(0),
        source_entered: Notify::new(),
        source_release: Notify::new(),
        dependent_entered: Notify::new(),
        dependent_release: Notify::new(),
    });
    let root = Context::new();
    let source = root
        .spawn(PreparedPlugin::from_input(Source(signals.clone()), 1))
        .await
        .unwrap();
    let dependent = root
        .spawn(PreparedPlugin::from_input(Dependent(signals.clone()), ()))
        .await
        .unwrap();
    assert_eq!(dependent.state(), FiberState::Pending);

    let restarting = tokio::spawn({
        let source = source.clone();
        async move { source.restart().await }
    });
    signals.source_entered.notified().await;
    // Preflight happens before restart publishes its service; slot claim waits.
    let swapping = source.era_swap(PreparedChange::from_input::<Source>(3));
    tokio::pin!(swapping);
    assert!(futures::poll!(&mut swapping).is_pending());
    signals.source_release.notify_one();
    restarting.await.unwrap().unwrap();
    signals.dependent_entered.notified().await;
    assert_eq!(dependent.state(), FiberState::Loading);

    let raced = tokio::time::timeout(std::time::Duration::from_millis(500), &mut swapping).await;
    let observed = dependent.state();
    signals.dependent_release.notify_one();
    if let Ok(result) = raced {
        let successor = result.unwrap();
        panic!(
            "era_swap returned with dependent state {observed:?} (successor {:?})",
            successor.state()
        );
    }
    swapping.await.unwrap();
    assert_eq!(dependent.ready().await.unwrap(), FiberState::Pending);
}
