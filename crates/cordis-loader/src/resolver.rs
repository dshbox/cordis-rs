//! Synchronous target-specific Loader resolution and typed JSON adaptation.
//!
//! Resolution is a pre-lifecycle firewall. [`PluginResolver`] receives an
//! opaque [`crate::resolver::PluginRequest`] containing only resolution identity and declarative
//! source input. A successful resolver returns a fully prepared, dependency-
//! overlaid, and sealed [`cordis_core::PreparedPlugin`]. Loader normalizes a returned resolver
//! error or panic at the crate-private invocation boundary before any lifecycle
//! admission exists.

use std::error::Error;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};

use cordis_core::{ConfigurableService, Plugin, PreparedPlugin};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::plan::{InjectEntry, PluginEntry};

/// The only source information visible to one target-specific resolution.
///
/// The request deliberately exposes exactly the entry's resolve key, JSON
/// Plugin configuration, and inject syntax. It has no EntryId, display/source
/// name, isolate policy, Context, Scope, realm, plan topology, or Runtime state.
/// All access is borrowed and read-only; the resolver cannot retain backing
/// plan representation through this value.
pub struct PluginRequest<'a> {
    resolve_key: &'a str,
    config: &'a Value,
    inject: &'a [InjectEntry],
}

impl<'a> PluginRequest<'a> {
    pub(crate) fn from_entry(entry: &'a PluginEntry) -> Self {
        let resolve_key = entry
            .key
            .as_deref()
            .or(entry.name.as_deref())
            .expect("validated Plugin entries always carry a resolve identity");
        Self {
            resolve_key,
            config: &entry.config,
            inject: &entry.inject,
        }
    }

    /// The exact Loader resolution identity: `key`, falling back to `name`.
    pub fn resolve_key(&self) -> &str {
        self.resolve_key
    }

    /// The Plugin's raw declarative JSON, valid only at this Loader adapter seam.
    pub fn config(&self) -> &Value {
        self.config
    }

    /// The entry's declarative dependency syntax for target-specific adaptation.
    pub fn inject(&self) -> &[InjectEntry] {
        self.inject
    }
}

/// Externally implemented synchronous resolution of one Loader Plugin target.
///
/// `Some` is a semantic promise: all target-specific Plugin and configured-
/// Service source has been typed and prepared, the entry dependency overlay has
/// been completed, and the returned [`PreparedPlugin`] is sealed. `None` means
/// only that [`PluginRequest::resolve_key`] is unknown. `Err` is a typed
/// preparation/resolution failure. Loader contains and normalizes `Err` and a
/// resolver panic only when it invokes this seam; direct typed preparation
/// helpers retain their ordinary Rust error and panic behavior.
///
/// This trait is intentionally synchronous and has no Context/topology input.
pub trait PluginResolver {
    /// Resolver-owned typed failure before Loader normalization.
    type Error: Error;

    /// Resolve and fully prepare one target without lifecycle admission.
    fn resolve(&self, request: PluginRequest<'_>) -> Result<Option<PreparedPlugin>, Self::Error>;
}

impl<F, E> PluginResolver for F
where
    F: for<'a> Fn(PluginRequest<'a>) -> Result<Option<PreparedPlugin>, E>,
    E: Error,
{
    type Error = E;

    fn resolve(&self, request: PluginRequest<'_>) -> Result<Option<PreparedPlugin>, Self::Error> {
        self(request)
    }
}

/// Semantic kind of one Loader-normalized resolver failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResolverFailureKind {
    /// The resolver returned its typed `Err` value.
    ReturnedError,
    /// The resolver panicked while adapting, preparing, overlaying, or sealing.
    Panic,
}

/// Opaque normalized failure of one resolver invocation.
///
/// The original error/panic object is not retained. Consumers may inspect only
/// its semantic [`ResolverFailureKind`] and owned diagnostic text.
#[derive(Debug)]
pub struct ResolverFailure {
    kind: ResolverFailureKind,
    diagnostic: String,
}

impl ResolverFailure {
    fn returned(diagnostic: String) -> Self {
        Self {
            kind: ResolverFailureKind::ReturnedError,
            diagnostic,
        }
    }

    fn panicked(diagnostic: String) -> Self {
        Self {
            kind: ResolverFailureKind::Panic,
            diagnostic,
        }
    }

    /// Whether the resolver returned an error or panicked.
    pub fn kind(&self) -> ResolverFailureKind {
        self.kind
    }

    /// The returned error's `Display` text or rendered panic payload.
    pub fn diagnostic(&self) -> &str {
        &self.diagnostic
    }
}

impl fmt::Display for ResolverFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ResolverFailureKind::ReturnedError => {
                write!(f, "resolver returned an error: {}", self.diagnostic)
            }
            ResolverFailureKind::Panic => write!(f, "resolver panicked: {}", self.diagnostic),
        }
    }
}

impl Error for ResolverFailure {}

#[derive(Debug)]
enum JsonPrepareErrorInner<E> {
    Deserialize(serde_json::Error),
    Prepare(E),
}

/// Typed failure while adapting JSON into one Plugin or ConfigurableService.
///
/// This error is deliberately not a normalized [`ResolverFailure`]. Direct
/// helper calls preserve the concrete preparation error inside
/// `JsonPrepareError<E>`, and direct preparation panics unwind normally. Only a resolver
/// invocation later normalizes a returned error or panic.
#[derive(Debug)]
pub struct JsonPrepareError<E: Error> {
    inner: JsonPrepareErrorInner<E>,
}

impl<E: Error> JsonPrepareError<E> {
    fn deserialize(error: serde_json::Error) -> Self {
        Self {
            inner: JsonPrepareErrorInner::Deserialize(error),
        }
    }

    fn prepare(error: E) -> Self {
        Self {
            inner: JsonPrepareErrorInner::Prepare(error),
        }
    }

    /// Return the concrete typed preparation error, when preparation (rather
    /// than JSON deserialization) failed.
    ///
    /// `Plugin::PrepareError` and `ConfigurableService::PrepareError`
    /// intentionally have no universal `'static` bound. Consequently the
    /// standard [`Error::source`] trait object cannot expose every possible
    /// `E`; this typed accessor preserves that weaker public bound without
    /// discarding access to the original error.
    pub fn prepare_error(&self) -> Option<&E> {
        match &self.inner {
            JsonPrepareErrorInner::Deserialize(_) => None,
            JsonPrepareErrorInner::Prepare(error) => Some(error),
        }
    }
}

impl<E: Error> fmt::Display for JsonPrepareError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.inner {
            JsonPrepareErrorInner::Deserialize(error) => {
                write!(f, "JSON source deserialization failed: {error}")
            }
            JsonPrepareErrorInner::Prepare(error) => {
                write!(f, "typed source preparation failed: {error}")
            }
        }
    }
}

impl<E: Error> Error for JsonPrepareError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.inner {
            JsonPrepareErrorInner::Deserialize(error) => Some(error),
            JsonPrepareErrorInner::Prepare(_) => None,
        }
    }
}

/// Deserialize, synchronously prepare, and seal one typed Plugin source.
///
/// JSON exists only for this Loader adapter call. Deserialization or a returned
/// [`Plugin::prepare`] error yields [`JsonPrepareError`]; a direct preparation,
/// declaration, or sealing panic remains an ordinary unwind. No lifecycle
/// operation is performed.
pub fn prepare_plugin_json<P>(
    plugin: P,
    config: &Value,
) -> Result<PreparedPlugin, JsonPrepareError<P::PrepareError>>
where
    P: Plugin,
    P::Config: DeserializeOwned,
{
    let typed = serde_json::from_value::<P::Config>(config.clone())
        .map_err(JsonPrepareError::deserialize)?;
    let input = plugin.prepare(typed).map_err(JsonPrepareError::prepare)?;
    Ok(PreparedPlugin::from_input(plugin, input))
}

/// Deserialize and synchronously prepare one ConfigurableService layer.
///
/// The returned `S::Layer` is typed semantic input suitable for
/// [`cordis_core::InjectSpec::require_configured`]. JSON never enters core.
/// A direct [`ConfigurableService::prepare_config`] panic unwinds normally.
pub fn prepare_service_json<S>(
    config: &Value,
) -> Result<S::Layer, JsonPrepareError<S::PrepareError>>
where
    S: ConfigurableService,
    S::Config: DeserializeOwned,
{
    let typed = serde_json::from_value::<S::Config>(config.clone())
        .map_err(JsonPrepareError::deserialize)?;
    S::prepare_config(typed).map_err(JsonPrepareError::prepare)
}

/// Invoke one resolver at Loader's normalization boundary.
///
/// There is intentionally no Context or lifecycle capability in this call. A
/// The execution layer translates `Ok(None)` into `UnresolvedKey`; this layer
/// owns only target preparation and failure normalization.
pub(crate) fn resolve_entry<R: PluginResolver + ?Sized>(
    entry: &PluginEntry,
    resolver: &R,
) -> Result<Option<PreparedPlugin>, ResolverFailure> {
    let request = PluginRequest::from_entry(entry);
    match catch_unwind(AssertUnwindSafe(|| {
        resolver.resolve(request).map_err(|error| error.to_string())
    })) {
        Ok(Ok(prepared)) => Ok(prepared),
        Ok(Err(diagnostic)) => Err(ResolverFailure::returned(diagnostic)),
        Err(payload) => {
            let diagnostic = if let Some(message) = payload.downcast_ref::<&'static str>() {
                (*message).to_owned()
            } else if let Some(message) = payload.downcast_ref::<String>() {
                message.clone()
            } else {
                "non-string panic payload".to_owned()
            };
            Err(ResolverFailure::panicked(diagnostic))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::InjectEntry;
    use cordis_core::Service;
    use std::convert::Infallible;
    use std::future::{Future, ready};

    fn entry(key: Option<&str>, name: Option<&str>) -> PluginEntry {
        PluginEntry {
            key: key.map(str::to_owned),
            name: name.map(str::to_owned),
            config: serde_json::json!({"enabled": true}),
            disabled: false,
            inject: vec![InjectEntry::Required("db".into())],
            isolate: Vec::new(),
        }
    }

    #[derive(Debug)]
    struct ResolverError(&'static str);
    impl fmt::Display for ResolverError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.0)
        }
    }
    impl Error for ResolverError {}

    #[test]
    fn request_uses_key_then_name_and_exposes_only_borrowed_resolution_source() {
        for (key, name, expected) in [
            (Some("explicit"), Some("display"), "explicit"),
            (None, Some("fallback"), "fallback"),
        ] {
            let row = entry(key, name);
            let seen = std::cell::RefCell::new(None::<(String, Value, usize)>);
            let resolver =
                |request: PluginRequest<'_>| -> Result<Option<PreparedPlugin>, ResolverError> {
                    *seen.borrow_mut() = Some((
                        request.resolve_key().to_owned(),
                        request.config().clone(),
                        request.inject().len(),
                    ));
                    Ok(None)
                };
            assert!(resolve_entry(&row, &resolver).unwrap().is_none());
            let seen = seen.into_inner().unwrap();
            assert_eq!(seen.0, expected);
            assert_eq!(seen.1, serde_json::json!({"enabled": true}));
            assert_eq!(seen.2, 1);
        }
    }

    #[test]
    fn unknown_key_remains_absence_for_the_execution_layer_to_classify() {
        let row = entry(Some("missing"), Some("ignored"));
        let resolver =
            |_: PluginRequest<'_>| -> Result<Option<PreparedPlugin>, ResolverError> { Ok(None) };
        assert!(resolve_entry(&row, &resolver).unwrap().is_none());
    }

    #[test]
    fn returned_error_normalizes_once_to_opaque_resolver_failure() {
        let runtime = cordis_core::Context::new();
        let before = runtime.runtime_snapshot();
        let row = entry(Some("broken"), None);
        let resolver = |_: PluginRequest<'_>| -> Result<Option<PreparedPlugin>, ResolverError> {
            Err(ResolverError("typed resolver failure"))
        };
        let failure = match resolve_entry(&row, &resolver) {
            Err(failure) => failure,
            Ok(_) => panic!("expected resolver failure"),
        };
        assert_eq!(failure.kind(), ResolverFailureKind::ReturnedError);
        assert_eq!(failure.diagnostic(), "typed resolver failure");
        assert!(failure.to_string().contains(failure.diagnostic()));
        let after = runtime.runtime_snapshot();
        assert_eq!(after.fibers().len(), before.fibers().len());
        assert_eq!(after.services().len(), before.services().len());
        assert_eq!(after.fibers().len(), 1);
    }

    #[test]
    fn resolver_panic_normalizes_once_without_crossing_lifecycle() {
        let runtime = cordis_core::Context::new();
        let before = runtime.runtime_snapshot();
        let row = entry(Some("panic"), None);
        let resolver = |_: PluginRequest<'_>| -> Result<Option<PreparedPlugin>, ResolverError> {
            panic!("resolver exploded")
        };
        let failure = match resolve_entry(&row, &resolver) {
            Err(failure) => failure,
            Ok(_) => panic!("expected resolver failure"),
        };
        assert_eq!(failure.kind(), ResolverFailureKind::Panic);
        assert_eq!(failure.diagnostic(), "resolver exploded");
        let after = runtime.runtime_snapshot();
        assert_eq!(after.fibers().len(), before.fibers().len());
        assert_eq!(after.services().len(), before.services().len());
        assert_eq!(after.fibers().len(), 1);
    }

    struct PanicPrepare;
    impl Plugin for PanicPrepare {
        type Config = serde_json::Value;
        type Input = ();
        type PrepareError = Infallible;
        type ApplyError = Infallible;
        fn prepare(&self, _: Self::Config) -> Result<Self::Input, Self::PrepareError> {
            panic!("prepare exploded")
        }
        fn apply(
            &self,
            _: cordis_core::Context,
            _: &(),
        ) -> impl Future<Output = Result<(), Infallible>> + Send {
            ready(Ok(()))
        }
    }

    #[derive(serde::Deserialize)]
    struct TypedPluginConfig {
        value: u8,
    }

    struct PreparedTarget;
    impl Plugin for PreparedTarget {
        type Config = TypedPluginConfig;
        type Input = u8;
        type PrepareError = Infallible;
        type ApplyError = Infallible;

        fn inject(&self) -> cordis_core::InjectSpec {
            cordis_core::InjectSpec::none().require("plugin-base")
        }

        fn prepare(&self, config: Self::Config) -> Result<Self::Input, Self::PrepareError> {
            Ok(config.value)
        }

        fn apply(
            &self,
            _: cordis_core::Context,
            _: &Self::Input,
        ) -> impl Future<Output = Result<(), Self::ApplyError>> + Send {
            ready(Ok(()))
        }
    }

    struct ConfiguredDependency;
    impl cordis_core::Service for ConfiguredDependency {
        const NAME: &'static str = "configured-dependency";
    }

    #[derive(serde::Deserialize)]
    struct DependencyConfig {
        value: u8,
    }

    impl ConfigurableService for ConfiguredDependency {
        type Config = DependencyConfig;
        type Layer = u8;
        type Resolved = u8;
        type PrepareError = Infallible;
        type ComposeError = Infallible;

        fn prepare_config(config: Self::Config) -> Result<Self::Layer, Self::PrepareError> {
            Ok(config.value)
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
    fn recognized_target_prepares_configured_inject_overlays_and_seals_before_any_admission() {
        let mut row = entry(Some("typed"), Some("display-only"));
        row.config = serde_json::json!({"value": 9});
        row.inject = vec![
            InjectEntry::Required("plain".into()),
            InjectEntry::Configured {
                service: ConfiguredDependency::NAME.into(),
                config: serde_json::json!({"value": 7}),
            },
        ];
        let runtime = cordis_core::Context::new();
        let before = runtime.runtime_snapshot();
        assert_eq!(before.fibers().len(), 1);
        assert!(before.services().is_empty());

        let resolver = |request: PluginRequest<'_>| -> Result<
            Option<PreparedPlugin>,
            JsonPrepareError<Infallible>,
        > {
            let mut overlay = cordis_core::InjectSpec::none();
            for inject in request.inject() {
                overlay = match inject {
                    InjectEntry::Required(service) => overlay.require(service.clone()),
                    InjectEntry::Configured { service, config }
                        if service == ConfiguredDependency::NAME =>
                    {
                        let layer = prepare_service_json::<ConfiguredDependency>(config)?;
                        overlay.require_configured::<ConfiguredDependency>(layer)
                    }
                    InjectEntry::Configured { .. } => unreachable!("fixture has one typed service"),
                };
            }
            let prepared = prepare_plugin_json(PreparedTarget, request.config())?
                .with_inject_overlay(overlay);
            Ok(Some(prepared))
        };

        assert!(resolve_entry(&row, &resolver).unwrap().is_some());
        let after = runtime.runtime_snapshot();
        assert_eq!(
            after.fibers().len(),
            1,
            "resolver success performs no Fiber admission"
        );
        assert!(
            after.services().is_empty(),
            "resolver success publishes no Service"
        );
    }

    struct RejectedDependency;
    impl cordis_core::Service for RejectedDependency {
        const NAME: &'static str = "rejected-dependency";
    }

    impl ConfigurableService for RejectedDependency {
        type Config = DependencyConfig;
        type Layer = u8;
        type Resolved = u8;
        type PrepareError = ResolverError;
        type ComposeError = Infallible;

        fn prepare_config(config: Self::Config) -> Result<Self::Layer, Self::PrepareError> {
            if config.value == 0 {
                Err(ResolverError("configured dependency rejected"))
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
    fn configured_service_prepare_error_normalizes_before_any_admission() {
        let mut row = entry(Some("configured-error"), None);
        row.inject = vec![InjectEntry::Configured {
            service: RejectedDependency::NAME.into(),
            config: serde_json::json!({"value": 0}),
        }];
        let runtime = cordis_core::Context::new();
        let before = runtime.runtime_snapshot();

        let resolver = |request: PluginRequest<'_>| -> Result<
            Option<PreparedPlugin>,
            JsonPrepareError<ResolverError>,
        > {
            for inject in request.inject() {
                if let InjectEntry::Configured { service, config } = inject
                    && service == RejectedDependency::NAME
                {
                    let _ = prepare_service_json::<RejectedDependency>(config)?;
                }
            }
            Ok(None)
        };
        let failure = match resolve_entry(&row, &resolver) {
            Err(failure) => failure,
            Ok(_) => panic!("configured Service rejection unexpectedly resolved"),
        };

        assert_eq!(failure.kind(), ResolverFailureKind::ReturnedError);
        assert!(
            failure
                .diagnostic()
                .contains("configured dependency rejected")
        );
        let after = runtime.runtime_snapshot();
        assert_eq!(after.fibers().len(), before.fibers().len());
        assert_eq!(after.services().len(), before.services().len());
        assert_eq!(after.fibers().len(), 1);
    }

    #[test]
    fn invalid_json_source_is_a_resolver_failure_with_zero_runtime_admission() {
        let mut row = entry(Some("bad-json"), None);
        row.config = serde_json::json!({"value": "not-a-number"});
        let runtime = cordis_core::Context::new();
        let before = runtime.runtime_snapshot();

        let resolver = |request: PluginRequest<'_>| -> Result<
            Option<PreparedPlugin>,
            JsonPrepareError<Infallible>,
        > {
            prepare_plugin_json(PreparedTarget, request.config()).map(Some)
        };
        let failure = match resolve_entry(&row, &resolver) {
            Err(failure) => failure,
            Ok(_) => panic!("invalid declarative source unexpectedly resolved"),
        };

        assert_eq!(failure.kind(), ResolverFailureKind::ReturnedError);
        assert!(
            failure
                .diagnostic()
                .contains("JSON source deserialization failed")
        );
        let after = runtime.runtime_snapshot();
        assert_eq!(after.fibers().len(), before.fibers().len());
        assert_eq!(after.services().len(), before.services().len());
        assert_eq!(after.fibers().len(), 1, "only the permanent root exists");
    }

    #[test]
    fn helper_panic_inside_resolver_normalizes_at_the_resolver_boundary() {
        let row = entry(Some("panic-prepare"), None);
        let resolver = |request: PluginRequest<'_>| -> Result<Option<PreparedPlugin>, JsonPrepareError<Infallible>> {
            prepare_plugin_json(PanicPrepare, request.config()).map(Some)
        };
        let failure = match resolve_entry(&row, &resolver) {
            Err(failure) => failure,
            Ok(_) => panic!("expected resolver failure"),
        };
        assert_eq!(failure.kind(), ResolverFailureKind::Panic);
        assert_eq!(failure.diagnostic(), "prepare exploded");
    }
}
