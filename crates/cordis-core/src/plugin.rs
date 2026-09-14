//! Typed Plugin preparation, sealing, and dependency declarations.
//!
//! Source configuration is prepared synchronously before lifecycle admission.
//! [`PreparedPlugin`] then seals one Plugin together with one value of its
//! declared [`Plugin::Input`] contract and materialized declarations; only [`Context::spawn`] begins
//! lifecycle work.

use crate::Context;
use crate::effect::BoxFuture;
use std::any::{Any, TypeId};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::error::Error;
use std::future::Future;
use std::sync::Arc;

/// A prepared, typed configuration layer retained for one Service contract.
#[derive(Clone)]
pub(crate) struct ConfiguredLayer {
    service_type: TypeId,
    value: Arc<dyn Any + Send + Sync>,
}

impl ConfiguredLayer {
    pub(crate) fn new<S: crate::Service + 'static, L: Send + Sync + 'static>(value: L) -> Self {
        Self {
            service_type: TypeId::of::<S>(),
            value: Arc::new(value),
        }
    }

    pub(crate) fn matches<S: crate::Service + 'static>(&self) -> bool {
        self.service_type == TypeId::of::<S>()
    }

    pub(crate) fn downcast_ref<L: 'static>(&self) -> Option<&L> {
        self.value.downcast_ref()
    }
}

/// One normalized required-Service declaration.
#[derive(Clone)]
pub(crate) struct InjectEntry {
    pub(crate) name: String,
    pub(crate) configured: Option<ConfiguredLayer>,
}

/// An immutable, normalized set of required Services.
///
/// There is at most one effective row for each Service name. Both builders are
/// consuming upserts: the later row wins, and [`InjectSpec::require`] clears a
/// previously configured layer for the same Service.
#[derive(Clone, Default)]
#[must_use = "an InjectSpec has no effect until it is sealed into a PreparedPlugin"]
pub struct InjectSpec {
    entries: BTreeMap<String, InjectEntry>,
}

impl std::fmt::Debug for InjectSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entries(
                self.entries
                    .values()
                    .map(|entry| (&entry.name, entry.configured.is_some())),
            )
            .finish()
    }
}

impl InjectSpec {
    /// Return the semantic empty dependency declaration.
    pub fn none() -> Self {
        Self::default()
    }

    /// Require `name`, replacing and clearing any earlier configured row.
    pub fn require(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        self.entries.insert(
            name.clone(),
            InjectEntry {
                name,
                configured: None,
            },
        );
        self
    }

    /// Require configurable Service `S` with an already-prepared typed layer.
    pub fn require_configured<S: crate::ConfigurableService>(mut self, layer: S::Layer) -> Self {
        let name = S::NAME.to_owned();
        self.entries.insert(
            name.clone(),
            InjectEntry {
                name,
                configured: Some(ConfiguredLayer::new::<S, _>(layer)),
            },
        );
        self
    }

    pub(crate) fn overlay(mut self, overlay: Self) -> Self {
        self.entries.extend(overlay.entries);
        self
    }

    pub(crate) fn entries(&self) -> impl Iterator<Item = &InjectEntry> {
        self.entries.values()
    }
}

/// A reusable behavior contract with synchronous typed preparation.
pub trait Plugin: Send + 'static {
    /// Consumer-facing source configuration.
    type Config;
    /// Semantically complete apply input retained by Cordis.
    type Input: Send + 'static;
    /// Synchronous source-preparation failure.
    type PrepareError: Error;
    /// Failure returned by one apply attempt.
    type ApplyError: Error;

    /// Return the diagnostic Plugin name.
    fn name(&self) -> Cow<'_, str> {
        Cow::Borrowed(std::any::type_name::<Self>())
    }

    /// Return this Plugin's required-Service declarations.
    fn inject(&self) -> InjectSpec {
        InjectSpec::none()
    }

    /// Synchronously validate and prepare source configuration.
    ///
    /// Runs before any lifecycle admission: an `Err` is the caller's own
    /// typed value and a panic is an ordinary unwind — nothing is
    /// normalized and no Runtime state exists yet.
    fn prepare(&self, config: Self::Config) -> Result<Self::Input, Self::PrepareError>;

    /// Apply one Plugin input to `ctx`.
    ///
    /// A returned error or a panic is normalized exactly once into the
    /// opaque [`PluginFailure`](crate::lifecycle::PluginFailure) (kind plus owned
    /// diagnostic text) at the framework boundary: for the initial apply
    /// it becomes [`SpawnError::InitialApply`](crate::lifecycle::SpawnError) with
    /// the attempted Fiber fully rolled back and unlinked; for later
    /// applies it parks the Fiber `Failed` with its generation rolled
    /// back.
    fn apply(
        &self,
        ctx: Context,
        input: &Self::Input,
    ) -> impl Future<Output = Result<(), Self::ApplyError>> + Send;
}

impl<P: Plugin + Sync> Plugin for Arc<P> {
    type Config = P::Config;
    type Input = P::Input;
    type PrepareError = P::PrepareError;
    type ApplyError = P::ApplyError;

    fn name(&self) -> Cow<'_, str> {
        (**self).name()
    }

    fn inject(&self) -> InjectSpec {
        (**self).inject()
    }

    fn prepare(&self, config: Self::Config) -> Result<Self::Input, Self::PrepareError> {
        (**self).prepare(config)
    }

    fn apply(
        &self,
        ctx: Context,
        input: &Self::Input,
    ) -> impl Future<Output = Result<(), Self::ApplyError>> + Send {
        (**self).apply(ctx, input)
    }
}

pub(crate) trait SealedPlugin: Send + Sync {
    fn apply_boxed(
        self: Arc<Self>,
        ctx: Context,
    ) -> BoxFuture<Result<(), crate::fiber::PluginFailure>>;

    /// Replace the retained Plugin input with the change's candidate,
    /// leaving the Plugin behavior untouched.
    ///
    /// The caller holds the fiber's lifecycle slot, so no apply lease is
    /// outstanding and the state is home. A foreign-contract change is
    /// handed back with the state intact (the precommit check upstream of
    /// this call is the guarantee; this arm is the fail-closed backstop).
    fn swap_input(&self, change: PreparedChange) -> Result<(), PreparedChange>;

    /// Move the Plugin behavior into a fresh seal carrying the change's
    /// candidate — the era swap's successor. The old seal is left empty:
    /// its fiber is already disposed, so no apply can observe the vacancy.
    ///
    /// A foreign-contract change, or a source whose behavior was already moved
    /// into a successor (empty state), hands the change back instead of
    /// corrupting either side.
    fn into_successor(
        self: Arc<Self>,
        change: PreparedChange,
    ) -> Result<Arc<dyn SealedPlugin>, PreparedChange>;
}

struct TypedPlugin<P: Plugin> {
    state: parking_lot::Mutex<Option<(P, P::Input)>>,
}

struct StateLease<P: Plugin> {
    owner: Arc<TypedPlugin<P>>,
    state: Option<(P, P::Input)>,
}

impl<P: Plugin> Drop for StateLease<P> {
    fn drop(&mut self) {
        let state = self
            .state
            .take()
            .expect("a Plugin state lease owns its value");
        let replaced = self.owner.state.lock().replace(state);
        debug_assert!(replaced.is_none(), "a Plugin cannot apply concurrently");
        drop(replaced);
    }
}

impl<P: Plugin> SealedPlugin for TypedPlugin<P> {
    fn apply_boxed(
        self: Arc<Self>,
        ctx: Context,
    ) -> BoxFuture<Result<(), crate::fiber::PluginFailure>> {
        Box::pin(async move {
            let state = self
                .state
                .lock()
                .take()
                .expect("the lifecycle arbiter serializes Plugin apply");
            let lease = StateLease {
                owner: self,
                state: Some(state),
            };
            let (plugin, input) = lease
                .state
                .as_ref()
                .expect("the state lease remains populated during apply");
            // the last boundary that knows the error's type: the typed
            // ApplyError normalizes exactly once, here, into the opaque
            // PluginFailure — the original object, `Any` access, and
            // downcasts never escape (the `Error`-only bound is why the
            // value cannot ride the Send + Sync boxed-error channel)
            let result = plugin.apply(ctx, input).await;
            result.map_err(|error| crate::fiber::PluginFailure::returned(error.to_string()))
        })
    }

    fn swap_input(&self, change: PreparedChange) -> Result<(), PreparedChange> {
        let (contract, input) = change.into_parts();
        if contract != TypeId::of::<P>() {
            return Err(PreparedChange::from_parts(contract, input));
        }
        let input = match input.downcast::<P::Input>() {
            Ok(input) => *input,
            // the contract check above is exhaustive: from_input seals
            // exactly this pairing
            Err(input) => return Err(PreparedChange::from_parts(contract, input)),
        };
        let superseded = {
            let mut state = self.state.lock();
            match state.as_mut() {
                Some((_, current)) => std::mem::replace(current, input),
                // the lifecycle-slot guarantee failed: refuse, never corrupt.
                // Hand the candidate back; its destruction remains outside the
                // state critical section when the returned value is dropped.
                None => {
                    return Err(PreparedChange::from_parts(
                        contract,
                        Box::new(input) as Box<dyn Any + Send>,
                    ));
                }
            }
        };
        // Plugin input values are user-owned and may run arbitrary
        // Drop code. Two-phase replacement keeps that work outside the mutex.
        drop(superseded);
        Ok(())
    }

    fn into_successor(
        self: Arc<Self>,
        change: PreparedChange,
    ) -> Result<Arc<dyn SealedPlugin>, PreparedChange> {
        let (contract, input) = change.into_parts();
        if contract != TypeId::of::<P>() {
            return Err(PreparedChange::from_parts(contract, input));
        }
        let input = match input.downcast::<P::Input>() {
            Ok(input) => *input,
            Err(input) => return Err(PreparedChange::from_parts(contract, input)),
        };
        let taken = self.state.lock().take();
        match taken {
            Some((plugin, _superseded)) => Ok(Arc::new(TypedPlugin {
                state: parking_lot::Mutex::new(Some((plugin, input))),
            })),
            None => Err(PreparedChange::from_parts(
                contract,
                Box::new(input) as Box<dyn Any + Send>,
            )),
        }
    }
}

/// A move-only seal associating one Plugin with one value of its declared Input contract.
///
/// Construction synchronously materializes [`Plugin::name`] and
/// [`Plugin::inject`]. Runtime lifecycle paths use those sealed values and do
/// not call either declaration method again.
#[must_use = "a PreparedPlugin must be passed to Context::spawn to begin lifecycle work"]
pub struct PreparedPlugin {
    pub(crate) plugin: Arc<dyn SealedPlugin>,
    pub(crate) name: String,
    pub(crate) inject: InjectSpec,
    pub(crate) contract: TypeId,
}

impl PreparedPlugin {
    /// Seal `plugin` with one value of its declared [`Plugin::Input`] contract.
    pub fn from_input<P: Plugin>(plugin: P, input: P::Input) -> Self {
        let name = plugin.name().into_owned();
        let inject = plugin.inject();
        Self {
            plugin: Arc::new(TypedPlugin {
                state: parking_lot::Mutex::new(Some((plugin, input))),
            }),
            name,
            inject,
            contract: TypeId::of::<P>(),
        }
    }

    /// Apply a normalized Loader dependency overlay before lifecycle admission.
    pub fn with_inject_overlay(mut self, overlay: InjectSpec) -> Self {
        self.inject = self.inject.overlay(overlay);
        self
    }
}

/// A move-only, one-attempt prepared input replacement for update or era replacement.
///
/// The value contains only Plugin contract identity and one complete replacement
/// Input value. It contains no Plugin behavior, creation recipe, Context, FiberHandle,
/// or lifecycle authority.
#[must_use = "a PreparedChange is consumed by one update or era-replacement attempt"]
pub struct PreparedChange {
    pub(crate) contract: TypeId,
    pub(crate) input: Box<dyn Any + Send>,
}

impl PreparedChange {
    /// Seal one replacement [`Plugin::Input`] value for Plugin contract `P`.
    pub fn from_input<P: Plugin>(input: P::Input) -> Self {
        Self {
            contract: TypeId::of::<P>(),
            input: Box::new(input),
        }
    }

    /// The Plugin contract this candidate was sealed for.
    pub(crate) fn contract(&self) -> TypeId {
        self.contract
    }

    pub(crate) fn into_parts(self) -> (TypeId, Box<dyn Any + Send>) {
        (self.contract, self.input)
    }

    pub(crate) fn from_parts(contract: TypeId, input: Box<dyn Any + Send>) -> Self {
        Self { contract, input }
    }
}

#[cfg(test)]
mod critical_section_tests {
    use super::{PreparedChange, SealedPlugin, TypedPlugin};
    use crate::{Context, Plugin};
    use std::convert::Infallible;
    use std::future::ready;
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    struct PreparedDropProbe {
        entered: Option<mpsc::Sender<()>>,
        release: Option<mpsc::Receiver<()>>,
    }

    impl PreparedDropProbe {
        fn inert() -> Self {
            Self {
                entered: None,
                release: None,
            }
        }
    }

    impl Drop for PreparedDropProbe {
        fn drop(&mut self) {
            if let Some(entered) = self.entered.take() {
                entered.send(()).expect("drop observer is alive");
                self.release
                    .take()
                    .expect("blocking probe has a release receiver")
                    .recv()
                    .expect("drop release is sent");
            }
        }
    }

    struct DropPreparedPlugin;

    impl Plugin for DropPreparedPlugin {
        type Config = ();
        type Input = PreparedDropProbe;
        type PrepareError = Infallible;
        type ApplyError = Infallible;

        fn prepare(&self, (): ()) -> Result<Self::Input, Self::PrepareError> {
            Ok(PreparedDropProbe::inert())
        }

        fn apply(
            &self,
            _ctx: Context,
            _input: &Self::Input,
        ) -> impl Future<Output = Result<(), Self::ApplyError>> + Send {
            ready(Ok(()))
        }
    }

    #[test]
    fn prepared_replacement_releases_bookkeeping_before_superseded_drop() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let plugin = Arc::new(TypedPlugin {
            state: parking_lot::Mutex::new(Some((
                DropPreparedPlugin,
                PreparedDropProbe {
                    entered: Some(entered_tx),
                    release: Some(release_rx),
                },
            ))),
        });

        let swapping = Arc::clone(&plugin);
        let worker = std::thread::spawn(move || {
            let result = swapping.swap_input(PreparedChange::from_input::<DropPreparedPlugin>(
                PreparedDropProbe::inert(),
            ));
            assert!(result.is_ok(), "matching prepared change is accepted");
        });

        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("superseded input value reaches Drop");
        let bookkeeping_is_open = plugin.state.try_lock().is_some();
        release_tx.send(()).expect("release blocked Drop");
        worker.join().expect("replacement worker completes");

        assert!(
            bookkeeping_is_open,
            "a blocked user Drop must not retain the Plugin state critical section"
        );
    }
}
