//! Application-facade compile smoke test.

use cordis::{Context, Event, Plugin, PreparedPlugin, Routing};

struct Ping;
impl Event for Ping {
    const NAME: &'static str = "facade-smoke-ping";
    type Args = ();
    type Output = ();
}

struct NeverPlugin;
impl Plugin for NeverPlugin {
    type Config = ();
    type Input = ();
    type PrepareError = std::convert::Infallible;
    type ApplyError = std::convert::Infallible;

    fn prepare(&self, (): ()) -> Result<(), Self::PrepareError> {
        Ok(())
    }

    async fn apply(&self, _: Context, _: &()) -> Result<(), Self::ApplyError> {
        Ok(())
    }
}

#[test]
fn application_facade_exports_the_documented_root_surface() {
    let root = Context::new();
    let _same_runtime_root = root.root();
    let _routing = Routing::Unscoped;
    let _event_name = Ping::NAME;

    fn accepts_prepared(_: Option<PreparedPlugin>) {}
    accepts_prepared(None);

    // The generic bound itself pins `cordis::Plugin`; lifecycle behavior is
    // exercised by cordis-core and the runnable examples.
    fn pin_plugin_bound<P: Plugin>() {}
    pin_plugin_bound::<NeverPlugin>();
}
