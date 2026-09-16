//! Runtime-local Event contract and listener storage.

use super::listener::{ErasedAroundListener, ErasedListener, ListenerOptions};
use super::types::{
    DispatchError, Event, EventOperation, ListenerRegistrationId, ListenerRole, Routing,
};
use crate::context::{Context, ScopeNode};
use crate::gated::PublishStep;
use parking_lot::Mutex;
use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
#[error("event contract mismatch")]
pub(crate) struct EventContractMismatch;

#[derive(Clone, Copy, PartialEq, Eq)]
struct EventContract {
    args: TypeId,
    output: TypeId,
}

impl EventContract {
    fn of<E: Event>() -> Self {
        Self {
            args: TypeId::of::<E::Args>(),
            output: TypeId::of::<E::Output>(),
        }
    }
}

#[derive(Clone)]
#[doc(hidden)]
pub enum HookKind {
    Plain {
        role: ListenerRole,
        callback: ErasedListener,
    },
    Around(ErasedAroundListener),
}

impl HookKind {
    pub(crate) fn role(&self) -> ListenerRole {
        match self {
            Self::Plain { role, .. } => *role,
            Self::Around(_) => ListenerRole::Around,
        }
    }
}

pub(crate) struct Hook {
    pub(crate) id: ListenerRegistrationId,
    prepend: bool,
    global: bool,
    pub(crate) scope: Option<Arc<ScopeNode>>,
    once: bool,
    options: ListenerOptions,
    pub(crate) kind: HookKind,
}

impl Hook {
    pub(crate) fn new(
        ctx: &Context,
        kind: HookKind,
        options: ListenerOptions,
        id: ListenerRegistrationId,
    ) -> Self {
        Self {
            id,
            prepend: options.is_prepend(),
            global: options.is_global(),
            scope: ctx.scope.clone(),
            once: options.is_once(),
            options,
            kind,
        }
    }

    fn snap(&self) -> HookSnap {
        HookSnap {
            id: self.id.clone(),
            scope: self.scope.clone(),
            options: self.options,
            kind: self.kind.clone(),
        }
    }
}

pub(crate) struct HookSnap {
    pub(crate) id: ListenerRegistrationId,
    pub(crate) scope: Option<Arc<ScopeNode>>,
    pub(crate) options: ListenerOptions,
    pub(crate) kind: HookKind,
}

#[derive(Default)]
struct EventHooks {
    order: Vec<ListenerRegistrationId>,
    entries: HashMap<ListenerRegistrationId, Hook>,
}

impl EventHooks {
    fn add(&mut self, hook: Hook) {
        self.compact_if_sparse();
        let id = hook.id.clone();
        let prepend = hook.prepend;
        let replaced = self.entries.insert(id.clone(), hook);
        debug_assert!(replaced.is_none(), "listener identities are fresh");
        if prepend {
            self.order.insert(0, id);
        } else {
            self.order.push(id);
        }
    }

    fn remove(&mut self, id: &ListenerRegistrationId) -> Option<Hook> {
        self.entries.remove(id)
    }

    fn get(&self, id: &ListenerRegistrationId) -> Option<&Hook> {
        self.entries.get(id)
    }

    fn iter(&self) -> impl Iterator<Item = &Hook> {
        self.order.iter().filter_map(|id| self.entries.get(id))
    }

    fn compact_if_sparse(&mut self) {
        debug_assert!(self.order.len() >= self.entries.len());
        let stale = self.order.len() - self.entries.len();
        if stale < 32 || stale < self.entries.len() {
            return;
        }
        let entries = &self.entries;
        self.order.retain(|id| entries.contains_key(id));
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Default)]
struct EventState {
    contracts: HashMap<&'static str, EventContract>,
    hooks: HashMap<&'static str, EventHooks>,
}

#[derive(Default)]
pub(crate) struct EventStore {
    state: Mutex<EventState>,
}

impl EventStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn bind_locked<E: Event>(state: &mut EventState) -> Result<(), ()> {
        let contract = EventContract::of::<E>();
        match state.contracts.get(E::NAME) {
            Some(bound) if *bound != contract => Err(()),
            Some(_) => Ok(()),
            None => {
                state.contracts.insert(E::NAME, contract);
                Ok(())
            }
        }
    }

    fn add_reserved<E: Event>(&self, hook: Hook) -> Result<(), EventContractMismatch> {
        let mut state = self.state.lock();
        Self::bind_locked::<E>(&mut state).map_err(|()| EventContractMismatch)?;
        let list = state.hooks.entry(E::NAME).or_default();
        list.add(hook);
        Ok(())
    }

    pub(crate) fn remove(&self, name: &str, id: &ListenerRegistrationId) -> bool {
        let removed = {
            let mut state = self.state.lock();
            let Some(list) = state.hooks.get_mut(name) else {
                return false;
            };
            let Some(removed) = list.remove(id) else {
                return false;
            };
            if list.is_empty() {
                state.hooks.remove(name);
            }
            removed
        };
        drop(removed);
        true
    }

    pub(crate) fn claim(&self, name: &str, id: &ListenerRegistrationId) -> (bool, bool) {
        let claimed = {
            let mut state = self.state.lock();
            let Some(list) = state.hooks.get_mut(name) else {
                return (false, false);
            };
            let Some(hook) = list.get(id) else {
                return (false, false);
            };
            if !hook.once {
                return (true, false);
            }
            let claimed = list
                .remove(id)
                .expect("the claimed listener remains indexed under the store lock");
            if list.is_empty() {
                state.hooks.remove(name);
            }
            Some(claimed)
        };
        drop(claimed);
        (true, true)
    }

    pub(crate) fn preflight<E: Event>(
        &self,
        routing: &Routing,
        operation: EventOperation,
    ) -> Result<Vec<HookSnap>, DispatchError> {
        let mut state = self.state.lock();
        Self::bind_locked::<E>(&mut state)
            .map_err(|()| DispatchError::EventContractMismatch { event: E::NAME })?;
        let Some(list) = state.hooks.get_mut(E::NAME) else {
            return Ok(Vec::new());
        };
        list.compact_if_sparse();

        let eligible = |hook: &Hook| match routing {
            Routing::Unscoped => true,
            Routing::Scoped(scope) => {
                hook.global
                    || match (&hook.scope, &scope.layer) {
                        (None, _) => true,
                        (Some(_), None) => false,
                        (Some(registration), Some(dispatch)) => dispatch.reaches(registration),
                    }
            }
        };
        let compatible = |role| match operation {
            EventOperation::Emit | EventOperation::EmitParallel | EventOperation::Query => {
                matches!(role, ListenerRole::Observer | ListenerRole::Responder)
            }
            EventOperation::Waterfall => {
                matches!(role, ListenerRole::Mapper | ListenerRole::Around)
            }
        };

        if let Some(role) = list
            .iter()
            .filter(|hook| eligible(hook))
            .map(|hook| hook.kind.role())
            .find(|role| !compatible(*role))
        {
            return Err(DispatchError::IncompatibleRole { operation, role });
        }

        Ok(list
            .iter()
            .filter(|hook| eligible(hook))
            .map(Hook::snap)
            .collect())
    }
}

pub(crate) struct HookPublish<E: Event> {
    pub(crate) store: Arc<EventStore>,
    pub(crate) hook: Option<Hook>,
    pub(crate) _event: std::marker::PhantomData<fn(E) -> E>,
    pub(crate) contract_mismatch: bool,
}

impl<E: Event> PublishStep for HookPublish<E> {
    fn publish(&mut self) -> std::result::Result<(), crate::gated::PublishRefused> {
        let hook = self.hook.take().expect("HookPublish runs at most once");
        match self.store.add_reserved::<E>(hook) {
            Ok(()) => Ok(()),
            Err(_) => {
                self.contract_mismatch = true;
                Err(crate::gated::PublishRefused)
            }
        }
    }
}
