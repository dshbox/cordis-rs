//! The futures of the `cordis-core` async operations are `Send` under exactly
//! the conditions the normative interface promises (§Auto traits). Each check
//! is generic over the operation's own parameters with only the bounds its
//! signature already requires, so it covers every permitted instantiation.
//! Only `Send` is asserted: neither `Sync`, `Unpin`, nor `'static` is promised.

use std::future::Future;
use std::time::Duration;

use cordis_core::{
    Context, Event, FiberHandle, FiberState, Plugin, PreparedChange, PreparedPlugin, QueryOutcome,
    Routing,
    effect::EffectRegistration,
    event::{DispatchError, Next},
    lifecycle::UpdateNext,
};

fn assert_send<T: Send>(_: &T) {}

fn lifecycle(handle: &FiberHandle, change: PreparedChange, other: PreparedChange) {
    assert_send(&handle.ready());
    assert_send(&handle.wait_state(FiberState::Active, Duration::from_secs(1)));
    assert_send(&handle.restart());
    assert_send(&handle.update(change));
    assert_send(&handle.era_swap(other));
    assert_send(&handle.dispose());
}

fn creation_and_removal<P: Plugin>(ctx: &Context, prepared: PreparedPlugin) {
    assert_send(&ctx.spawn(prepared));
    assert_send(&ctx.remove_plugins::<P>());
}

fn effect(registration: EffectRegistration) {
    assert_send(&registration.dispose());
}

fn dispatch<E: Event>(ctx: &Context, routing: Routing, args: E::Args)
where
    E::Args: Clone,
{
    assert_send(&ctx.emit::<E>(routing.clone(), args.clone()));
    assert_send(&ctx.emit_parallel::<E>(routing.clone(), args.clone()));
    assert_send(&ctx.query::<E>(routing, args));
}

fn waterfall<E, F, Fut, Err>(ctx: &Context, routing: Routing, args: E::Args, tail: F)
where
    E: Event,
    F: FnOnce(E::Args) -> Fut + Send + 'static,
    Fut: Future<Output = Result<E::Output, Err>> + Send + 'static,
    Err: std::error::Error + 'static,
{
    assert_send(&ctx.waterfall::<E, F, Fut, Err>(routing, args, tail));
}

fn waterfall_query<E, Q, R, Fut, Err>(ctx: &Context, routing: Routing, args: E::Args, resolver: R)
where
    E: Event,
    Q: Event<Args = E::Args, Output = E::Output>,
    E::Args: Clone,
    R: FnOnce(Result<QueryOutcome<E::Output>, DispatchError>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<E::Output, Err>> + Send + 'static,
    Err: std::error::Error + 'static,
{
    assert_send(&ctx.waterfall_query::<E, Q, R, Fut, Err>(routing, args, resolver));
}

fn continuations<E: Event, P: Plugin>(
    next: Next<E>,
    args: E::Args,
    update_next: UpdateNext<P>,
    input: P::Input,
) {
    assert_send(&next.call(args));
    assert_send(&update_next.call(input));
}

fn main() {}
