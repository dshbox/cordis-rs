use std::error::Error;
use std::fmt;
use std::rc::Rc;
use cordis_core::{Context, Plugin, PreparedPlugin};
use cordis_loader::PluginResolver;
use cordis_loader::resolver::{PluginRequest, ResolverFailure, ResolverFailureKind, JsonPrepareError, prepare_plugin_json, prepare_service_json};

#[derive(Debug)] struct LocalError(Rc<()>);
impl fmt::Display for LocalError { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("local") } }
impl Error for LocalError {}

fn accept<R: PluginResolver>(_: &R) {}
fn inspect(request: PluginRequest<'_>) {
    let _: &str = request.resolve_key();
    let _: &serde_json::Value = request.config();
    let _: &[cordis_loader::plan::InjectEntry] = request.inject();
}
fn failure(f: &ResolverFailure) {
    let _: ResolverFailureKind = f.kind();
    let _: &str = f.diagnostic();
}
fn json_error<E: Error>(error: &JsonPrepareError<E>) {
    let _: Option<&E> = error.prepare_error();
}

fn main() {
    let state = Rc::new(());
    let resolver = move |_: PluginRequest<'_>| -> Result<Option<PreparedPlugin>, LocalError> {
        let _ = &state;
        Ok(None)
    };
    accept(&resolver);
    let _ = inspect;
    let _ = failure;
    let _ = json_error::<LocalError>;
    let plugin = prepare_plugin_json(DummyPlugin, &serde_json::Value::Null).unwrap();
    let _: PreparedPlugin = plugin;
    let _: Result<PreparedPlugin, JsonPrepareError<LocalError>> =
        prepare_plugin_json(NonSendErrorPlugin, &serde_json::Value::Null);
    let _: Result<(), JsonPrepareError<LocalError>> =
        prepare_service_json::<DummyService>(&serde_json::Value::Null);
}

struct DummyService;
impl cordis_core::Service for DummyService { const NAME: &'static str = "dummy"; }
impl cordis_core::ConfigurableService for DummyService {
    type Config = ();
    type Layer = ();
    type Resolved = ();
    type PrepareError = LocalError;
    type ComposeError = std::convert::Infallible;
    fn prepare_config(_: ()) -> Result<(), Self::PrepareError> { Err(LocalError(Rc::new(()))) }
    fn compose_config<'a>(_: Option<&'a ()>, _: impl IntoIterator<Item=&'a ()>, _: Option<&'a ()>) -> Result<(), Self::ComposeError> { Ok(()) }
}


struct DummyPlugin;
impl Plugin for DummyPlugin {
    type Config = ();
    type Input = ();
    type PrepareError = std::convert::Infallible;
    type ApplyError = std::convert::Infallible;
    fn prepare(&self, _: ()) -> Result<(), Self::PrepareError> { Ok(()) }
    fn apply(&self, _: Context, _: &()) -> impl std::future::Future<Output=Result<(), Self::ApplyError>> + Send { std::future::ready(Ok(())) }
}


struct NonSendErrorPlugin;
impl Plugin for NonSendErrorPlugin {
    type Config = ();
    type Input = ();
    type PrepareError = LocalError;
    type ApplyError = std::convert::Infallible;
    fn prepare(&self, _: ()) -> Result<(), Self::PrepareError> { Err(LocalError(Rc::new(()))) }
    fn apply(&self, _: Context, _: &()) -> impl std::future::Future<Output=Result<(), Self::ApplyError>> + Send { std::future::ready(Ok(())) }
}
