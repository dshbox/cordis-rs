//! Runtime/public-contract evidence for Issue 50's Loader resolver firewall.

use std::convert::Infallible;
use std::error::Error;
use std::fmt;
use std::future::{Future, ready};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use cordis_core::{ConfigurableService, Context, InjectSpec, Plugin, Service};
use cordis_loader::resolver::{JsonPrepareError, prepare_plugin_json, prepare_service_json};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug)]
struct PrepareError(&'static str);

impl fmt::Display for PrepareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl Error for PrepareError {}

#[derive(Deserialize)]
struct PluginConfig {
    value: u8,
}

struct PreparedProbe {
    prepares: Arc<AtomicUsize>,
    names: Arc<AtomicUsize>,
    injects: Arc<AtomicUsize>,
}

impl Plugin for PreparedProbe {
    type Config = PluginConfig;
    type Input = u8;
    type PrepareError = PrepareError;
    type ApplyError = Infallible;

    fn name(&self) -> std::borrow::Cow<'_, str> {
        self.names.fetch_add(1, Ordering::SeqCst);
        "prepared-probe".into()
    }

    fn inject(&self) -> InjectSpec {
        self.injects.fetch_add(1, Ordering::SeqCst);
        InjectSpec::none().require("base")
    }

    fn prepare(&self, config: Self::Config) -> Result<Self::Input, Self::PrepareError> {
        self.prepares.fetch_add(1, Ordering::SeqCst);
        if config.value == 0 {
            Err(PrepareError("plugin rejected zero"))
        } else {
            Ok(config.value)
        }
    }

    fn apply(
        &self,
        _ctx: Context,
        _input: &Self::Input,
    ) -> impl Future<Output = Result<(), Self::ApplyError>> + Send {
        ready(Ok(()))
    }
}

struct JsonService;
impl Service for JsonService {
    const NAME: &'static str = "json-service";
}

#[derive(Deserialize)]
struct ServiceConfig {
    value: u8,
}

impl ConfigurableService for JsonService {
    type Config = ServiceConfig;
    type Layer = u8;
    type Resolved = u8;
    type PrepareError = PrepareError;
    type ComposeError = Infallible;

    fn prepare_config(config: Self::Config) -> Result<Self::Layer, Self::PrepareError> {
        if config.value == 0 {
            Err(PrepareError("service rejected zero"))
        } else {
            Ok(config.value)
        }
    }

    fn compose_config<'a>(
        base: Option<&'a Self::Layer>,
        layers: impl IntoIterator<Item = &'a Self::Layer>,
        head: Option<&'a Self::Layer>,
    ) -> Result<Self::Resolved, Self::ComposeError> {
        Ok(base.into_iter().chain(layers).chain(head).copied().sum())
    }
}

#[test]
fn plugin_json_prepares_then_seals_synchronously() {
    let prepares = Arc::new(AtomicUsize::new(0));
    let names = Arc::new(AtomicUsize::new(0));
    let injects = Arc::new(AtomicUsize::new(0));
    let plugin = PreparedProbe {
        prepares: prepares.clone(),
        names: names.clone(),
        injects: injects.clone(),
    };

    let prepared = prepare_plugin_json(plugin, &json!({"value": 7})).unwrap();
    let _: cordis_core::PreparedPlugin = prepared;
    assert_eq!(prepares.load(Ordering::SeqCst), 1);
    assert_eq!(names.load(Ordering::SeqCst), 1);
    assert_eq!(injects.load(Ordering::SeqCst), 1);
}

#[test]
fn json_helpers_distinguish_deserialization_from_typed_preparation_without_panics() {
    let plugin = PreparedProbe {
        prepares: Arc::new(AtomicUsize::new(0)),
        names: Arc::new(AtomicUsize::new(0)),
        injects: Arc::new(AtomicUsize::new(0)),
    };
    let malformed = match prepare_plugin_json(plugin, &json!({"value": "wrong"})) {
        Err(error) => error,
        Ok(_) => panic!("malformed Plugin JSON unexpectedly prepared"),
    };
    assert!(malformed.to_string().contains("JSON"));
    assert!(malformed.source().is_some());
    assert!(malformed.prepare_error().is_none());

    let service = prepare_service_json::<JsonService>(&json!({"value": 0})).unwrap_err();
    assert!(service.to_string().contains("service rejected zero"));
    assert_eq!(
        service.prepare_error().map(ToString::to_string).as_deref(),
        Some("service rejected zero")
    );
    assert!(service.source().is_none());

    fn assert_error<E: Error>(_: &JsonPrepareError<E>) {}
    assert_error(&service);
}

struct PanicPlugin;
impl Plugin for PanicPlugin {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = Infallible;

    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        panic!("direct prepare panic")
    }
    fn apply(
        &self,
        _ctx: Context,
        _prepared: &(),
    ) -> impl Future<Output = Result<(), Infallible>> + Send {
        ready(Ok(()))
    }
}

struct PanicService;
impl Service for PanicService {
    const NAME: &'static str = "panic-service";
}
impl ConfigurableService for PanicService {
    type Config = ();
    type Layer = ();
    type Resolved = ();
    type PrepareError = Infallible;
    type ComposeError = Infallible;
    fn prepare_config(_: ()) -> Result<(), Infallible> {
        panic!("direct service prepare panic")
    }
    fn compose_config<'a>(
        _: Option<&'a ()>,
        _: impl IntoIterator<Item = &'a ()>,
        _: Option<&'a ()>,
    ) -> Result<(), Infallible> {
        Ok(())
    }
}

#[test]
fn direct_json_helper_panics_remain_ordinary_pre_lifecycle_unwinds() {
    let plugin =
        std::panic::catch_unwind(|| prepare_plugin_json(PanicPlugin, &serde_json::Value::Null));
    assert!(plugin.is_err());
    let service =
        std::panic::catch_unwind(|| prepare_service_json::<PanicService>(&serde_json::Value::Null));
    assert!(service.is_err());
}
