//! Typed same-Fiber update control.
//!
//! `Context::on_update` registers generation-owned Mapper/Around policy
//! occurrences. The framework alone invokes the scoped precommit chain for a
//! `FiberHandle::update`: matching layers may transform, veto, or fail the
//! request-local `PreparedChange`, and the private tail only records provisional
//! acceptance. Lifecycle admission and commit happen afterward.
//!
//! Registration reuses Event listener adapters, options, occurrence identities,
//! and the gated publication seam, but update control is not Event dispatch. In
//! particular, update-policy registrations and invocations are not
//! `RuntimeObservation::ListenerRegistration` / `DispatchCompleted` records;
//! ADRs 0034 and 0035 keep precommit policy separate from postcommit Runtime
//! observation.
use crate::Plugin;
use crate::context::{Context, ScopeNode};
use crate::events::{
    AroundAdapter, InvocationFailure, ListenerOptions, ListenerRegistration,
    ListenerRegistrationError, ListenerRegistrationId, MapperAdapter,
    fresh_listener_registration_id,
};
use crate::plugin::PreparedChange;
use futures::FutureExt;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    any::TypeId, collections::HashMap, error::Error, future::Future, marker::PhantomData,
    panic::AssertUnwindSafe, pin::Pin, sync::Arc,
};

type ControlFuture =
    Pin<Box<dyn Future<Output = Result<PreparedChange, InvocationFailure>> + Send>>;
type ControlNext = Box<dyn FnOnce(PreparedChange) -> ControlFuture + Send>;
type MapperFn = Arc<dyn Fn(PreparedChange) -> ControlFuture + Send + Sync>;
type AroundFn = Arc<dyn Fn(PreparedChange, ControlNext) -> ControlFuture + Send + Sync>;

#[derive(Clone)]
pub enum UpdateKind {
    Mapper(MapperFn),
    Around(AroundFn),
}
struct UpdateHook {
    id: ListenerRegistrationId,
    prepend: bool,
    global: bool,
    scope: Option<Arc<ScopeNode>>,
    once: bool,
    kind: UpdateKind,
}
#[derive(Clone)]
struct UpdateSnap {
    id: ListenerRegistrationId,
    kind: UpdateKind,
}
#[derive(Default)]
struct UpdateState {
    hooks: HashMap<TypeId, Vec<UpdateHook>>,
}
#[derive(Default)]
pub(crate) struct UpdateStore {
    state: Mutex<UpdateState>,
}
impl UpdateStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn add(&self, contract: TypeId, hook: UpdateHook) {
        let mut state = self.state.lock();
        let list = state.hooks.entry(contract).or_default();
        if hook.prepend {
            list.insert(0, hook);
        } else {
            list.push(hook);
        }
    }

    fn remove(&self, contract: TypeId, id: &ListenerRegistrationId) -> bool {
        let removed = {
            let mut state = self.state.lock();
            let Some(list) = state.hooks.get_mut(&contract) else {
                return false;
            };
            let Some(at) = list.iter().position(|hook| hook.id == *id) else {
                return false;
            };
            let removed = list.remove(at);
            if list.is_empty() {
                state.hooks.remove(&contract);
            }
            removed
        };
        drop(removed);
        true
    }

    fn claim(&self, contract: TypeId, id: &ListenerRegistrationId) -> bool {
        let claimed = {
            let mut state = self.state.lock();
            let Some(list) = state.hooks.get_mut(&contract) else {
                return false;
            };
            let Some(at) = list.iter().position(|hook| hook.id == *id) else {
                return false;
            };
            if !list[at].once {
                return true;
            }
            let claimed = list.remove(at);
            if list.is_empty() {
                state.hooks.remove(&contract);
            }
            Some(claimed)
        };
        drop(claimed);
        true
    }

    fn snapshot(&self, contract: TypeId, target: &Option<Arc<ScopeNode>>) -> Vec<UpdateSnap> {
        self.state
            .lock()
            .hooks
            .get(&contract)
            .into_iter()
            .flatten()
            .filter(|hook| {
                hook.global
                    || match (&hook.scope, target) {
                        (None, _) => true,
                        (Some(_), None) => false,
                        (Some(registration), Some(dispatch)) => dispatch.reaches(registration),
                    }
            })
            .map(|hook| UpdateSnap {
                id: hook.id.clone(),
                kind: hook.kind.clone(),
            })
            .collect()
    }

    pub(crate) async fn control(
        self: &Arc<Self>,
        contract: TypeId,
        target: Option<Arc<ScopeNode>>,
        change: PreparedChange,
    ) -> Result<Option<PreparedChange>, InvocationFailure> {
        let hooks = Arc::new(self.snapshot(contract, &target));
        let reached = Arc::new(AtomicBool::new(false));
        let output =
            Self::run_chain(self.clone(), contract, hooks, 0, change, reached.clone()).await?;
        Ok(reached.load(Ordering::Acquire).then_some(output))
    }

    fn run_chain(
        store: Arc<Self>,
        contract: TypeId,
        hooks: Arc<Vec<UpdateSnap>>,
        index: usize,
        change: PreparedChange,
        reached: Arc<AtomicBool>,
    ) -> ControlFuture {
        Box::pin(async move {
            if index == hooks.len() {
                reached.store(true, Ordering::Release);
                return Ok(change);
            }
            let hook = hooks[index].clone();
            if !store.claim(contract, &hook.id) {
                return Self::run_chain(store, contract, hooks, index + 1, change, reached).await;
            }
            let result = match hook.kind {
                UpdateKind::Mapper(callback) => match invoke_mapper(callback, change).await {
                    Ok(change) => {
                        Self::run_chain(store, contract, hooks, index + 1, change, reached).await
                    }
                    Err(failure) => Err(failure),
                },
                UpdateKind::Around(callback) => {
                    let next_store = store.clone();
                    let next_hooks = hooks.clone();
                    let next_reached = reached.clone();
                    let next: ControlNext = Box::new(move |change| {
                        Self::run_chain(
                            next_store,
                            contract,
                            next_hooks,
                            index + 1,
                            change,
                            next_reached,
                        )
                    });
                    invoke_around(callback, change, next).await
                }
            };
            result.map_err(|failure| failure.correlate(hook.id))
        })
    }
}

async fn invoke_mapper(
    callback: MapperFn,
    change: PreparedChange,
) -> Result<PreparedChange, InvocationFailure> {
    let future = std::panic::catch_unwind(AssertUnwindSafe(|| callback(change)))
        .map_err(|panic| InvocationFailure::panic(crate::events::panic_message(panic)))?;
    AssertUnwindSafe(future)
        .catch_unwind()
        .await
        .map_err(|panic| InvocationFailure::panic(crate::events::panic_message(panic)))?
}

async fn invoke_around(
    callback: AroundFn,
    change: PreparedChange,
    next: ControlNext,
) -> Result<PreparedChange, InvocationFailure> {
    let future = std::panic::catch_unwind(AssertUnwindSafe(|| callback(change, next)))
        .map_err(|panic| InvocationFailure::panic(crate::events::panic_message(panic)))?;
    AssertUnwindSafe(future)
        .catch_unwind()
        .await
        .map_err(|panic| InvocationFailure::panic(crate::events::panic_message(panic)))?
}

fn normalize_returned<Err: Error + 'static>(error: Err) -> InvocationFailure {
    let diagnostic = error.to_string();
    let erased: Box<dyn Error> = Box::new(error);
    match erased.downcast::<InvocationFailure>() {
        Ok(failure) => *failure,
        Err(_) => InvocationFailure::returned(diagnostic),
    }
}

fn take_input<P: Plugin>(change: PreparedChange) -> P::Input {
    let (contract, input) = change.into_parts();
    debug_assert_eq!(contract, TypeId::of::<P>());
    *input
        .downcast::<P::Input>()
        .expect("PreparedChange seals one Plugin contract with its Input type")
}

fn reseal<P: Plugin>(input: P::Input) -> PreparedChange {
    PreparedChange::from_input::<P>(input)
}

/// Consuming continuation handed to typed update Around policy.
pub struct UpdateNext<P: Plugin> {
    inner: ControlNext,
    _plugin: PhantomData<fn(P) -> P>,
}

impl<P: Plugin> UpdateNext<P> {
    fn new(inner: ControlNext) -> Self {
        Self {
            inner,
            _plugin: PhantomData,
        }
    }

    /// Invoke the remaining precommit update-control chain exactly once.
    pub async fn call(self, input: P::Input) -> Result<P::Input, InvocationFailure> {
        let change = (self.inner)(reseal::<P>(input)).await?;
        Ok(take_input::<P>(change))
    }
}

mod sealed {
    use super::{Context, UpdateKind};
    use crate::Plugin;
    pub trait UpdateListenerImpl<P: Plugin>: Send + Sync + 'static {
        fn into_update_kind(self, registration: Context) -> UpdateKind;
    }
}

/// Sealed, methodless typed update-policy capability.
pub trait UpdateListener<P: Plugin>: sealed::UpdateListenerImpl<P> {}
impl<P: Plugin, T> UpdateListener<P> for T where T: sealed::UpdateListenerImpl<P> {}

impl<P, C, Fut, Err> sealed::UpdateListenerImpl<P> for MapperAdapter<C, true, P>
where
    P: Plugin,
    C: Fn(Context, P::Input) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<P::Input, Err>> + Send + 'static,
    Err: Error + 'static,
{
    fn into_update_kind(self, registration: Context) -> UpdateKind {
        let callback = self.0;
        UpdateKind::Mapper(Arc::new(move |change| {
            let future = callback(registration.clone(), take_input::<P>(change));
            Box::pin(async move { future.await.map(reseal::<P>).map_err(normalize_returned) })
        }))
    }
}

impl<P, C, Err> sealed::UpdateListenerImpl<P> for MapperAdapter<C, false, P>
where
    P: Plugin,
    C: Fn(Context, P::Input) -> Result<P::Input, Err> + Send + Sync + 'static,
    Err: Error + 'static,
{
    fn into_update_kind(self, registration: Context) -> UpdateKind {
        let callback = self.0;
        UpdateKind::Mapper(Arc::new(move |change| {
            let result = callback(registration.clone(), take_input::<P>(change))
                .map(reseal::<P>)
                .map_err(normalize_returned);
            Box::pin(std::future::ready(result))
        }))
    }
}

impl<P, C, Fut, Err> sealed::UpdateListenerImpl<P> for AroundAdapter<C, P>
where
    P: Plugin,
    C: Fn(Context, P::Input, UpdateNext<P>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<P::Input, Err>> + Send + 'static,
    Err: Error + 'static,
{
    fn into_update_kind(self, registration: Context) -> UpdateKind {
        let callback = self.0;
        UpdateKind::Around(Arc::new(move |change, next| {
            let future = callback(
                registration.clone(),
                take_input::<P>(change),
                UpdateNext::new(next),
            );
            Box::pin(async move { future.await.map(reseal::<P>).map_err(normalize_returned) })
        }))
    }
}

struct UpdatePublish {
    store: Arc<UpdateStore>,
    contract: TypeId,
    hook: Option<UpdateHook>,
}

impl crate::gated::PublishStep for UpdatePublish {
    fn publish(&mut self) -> std::result::Result<(), crate::gated::PublishRefused> {
        self.store.add(
            self.contract,
            self.hook.take().expect("UpdatePublish runs once"),
        );
        Ok(())
    }
}

fn cleanup(
    store: Arc<UpdateStore>,
    contract: TypeId,
    id: ListenerRegistrationId,
) -> crate::effect::Cleanup {
    crate::effect::sync_cleanup(move || {
        store.remove(contract, &id);
    })
}

impl Context {
    /// Register typed precommit update policy. Update control has no public dispatch operation.
    pub fn on_update<P, L>(
        &self,
        listener: L,
        options: ListenerOptions,
    ) -> Result<ListenerRegistration, ListenerRegistrationError>
    where
        P: Plugin,
        L: UpdateListener<P>,
    {
        let store = self.root.updates.clone();
        let contract = TypeId::of::<P>();
        let id = fresh_listener_registration_id();
        let hook = UpdateHook {
            id: id.clone(),
            prepend: options.is_prepend(),
            global: options.is_global(),
            scope: self.scope.clone(),
            once: options.is_once(),
            kind: sealed::UpdateListenerImpl::into_update_kind(listener, self.clone()),
        };
        let fiber = self.fiber().clone();
        let mut publish = UpdatePublish {
            store: store.clone(),
            contract,
            hook: Some(hook),
        };
        let token = crate::gated::push_gated(
            &fiber,
            cleanup(store.clone(), contract, id.clone()),
            &mut publish,
        )
        .map_err(|_| ListenerRegistrationError::InactiveContext)?;
        let remove_store = store.clone();
        Ok(ListenerRegistration {
            remove: Arc::new(move |registration| remove_store.remove(contract, registration)),
            id,
            cleanup: token,
            owner: Arc::downgrade(&fiber),
        })
    }
}
