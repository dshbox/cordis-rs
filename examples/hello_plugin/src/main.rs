//! hello_plugin — the on-ramp seat: the smallest correct Cordis program.
//!
//! A [`Plugin`] prepares typed source configuration before lifecycle admission.
//! [`PreparedPlugin`] then seals that input value to the Plugin contract, and
//! [`Context::spawn`] performs the complete lifecycle transaction before handing
//! the consumer a live quiescent [`cordis_core::FiberHandle`]. The consumer keeps that
//! FiberHandle in its Harness roster and deterministically disposes it at teardown.
//!
//! Run with `cargo run -p hello_plugin`.

use std::convert::Infallible;

use cordis_core::event::{ListenerRegistrationError, observer_sync};
use cordis_core::{BoxError, Context, Event, Plugin, PreparedPlugin, Routing};
use examples_common::{Roster, section};

// 1. Declare an event: one Runtime-local name with one typed contract.
struct Ping;

impl Event for Ping {
    const NAME: &'static str = "ping";
    type Args = String;
    type Output = ();
}

// 2. Define a Plugin with explicit unit source configuration. Even unit
//    configuration is prepared into a typed input before lifecycle admission; no builder,
//    default config, erased config, or raw lifecycle input is involved.
struct Echo;

struct EchoInput;

impl Plugin for Echo {
    type Config = ();
    type Input = EchoInput;
    type PrepareError = Infallible;
    type ApplyError = ListenerRegistrationError;

    fn prepare(&self, (): ()) -> Result<EchoInput, Infallible> {
        Ok(EchoInput)
    }

    async fn apply(&self, ctx: Context, _input: &EchoInput) -> Result<(), Self::ApplyError> {
        // 3. Register typed Plugin behavior. The returned exact-occurrence
        // capability may be dropped inertly because the Fiber generation owns
        // cleanup; deterministic FiberHandle disposal removes the registration.
        let _listener = ctx.on::<Ping, _>(observer_sync(|_, name| {
            println!("  hello, {name}");
            Ok::<_, Infallible>(())
        }))?;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let ctx = Context::new();
    let mut roster = Roster::new();

    // 4. Prepare explicitly, seal the exact Plugin/input association, then
    //    perform complete spawn. A successful handoff is already live and
    //    quiescent; the Harness records the delivered FiberHandle for teardown.
    section("boot: one echo plugin");
    let plugin = Echo;
    let input = plugin.prepare(())?;
    let sealed = PreparedPlugin::from_input(plugin, input);
    roster.push(ctx.spawn(sealed).await?);
    roster.report().await;

    // 5. Dispatch is typed and routing is explicit. The Plugin behavior is
    //    observable as "hello, world".
    section("run: emit Ping");
    ctx.emit::<Ping>(Routing::Unscoped, "world".into()).await?;

    // 6. Consumer-owned teardown deterministically disposes the delivered FiberHandle.
    section("teardown: dispose the FiberHandle");
    roster.teardown().await;

    // The listener was generation-owned: after disposal there is no callback.
    ctx.emit::<Ping>(Routing::Unscoped, "nobody listens anymore".into())
        .await?;
    println!("  (the second Ping printed nothing — the listener died with the FiberHandle)");

    Ok(())
}
