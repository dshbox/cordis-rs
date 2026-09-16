//! Event registration and dispatch over named contracts, semantic listener roles,
//! and explicit Scope routing.

use super::listener::{
    CallbackValue, ErasedAroundListener, ErasedListener, Listener, ListenerOptions,
    ListenerRegistration, NextFn,
};
use super::store::{Hook, HookKind, HookSnap};
use super::types::{
    DispatchError, ErasedPayload, Event, EventOperation, InvocationFailure,
    ListenerRegistrationError, ListenerRegistrationId, ParallelFailures, QueryOutcome, Routing,
    panic_message,
};
use crate::context::{Context, Root};
use futures::{FutureExt, future::join_all};
use std::error::Error;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

#[derive(Clone)]
struct ListenerObservationMeta {
    id: ListenerRegistrationId,
    event: &'static str,
    role: super::ListenerRole,
    scope: crate::observation::ScopeId,
    options: ListenerOptions,
}

fn publish_listener_change(
    root: &Root,
    change: crate::observation::ListenerChange,
    meta: &ListenerObservationMeta,
) {
    root.observations.publish(
        crate::observation::RuntimeObservation::ListenerRegistration {
            change,
            listener: meta.id.clone(),
            event: meta.event,
            role: meta.role,
            scope: meta.scope.clone(),
            options: meta.options,
        },
    );
}

fn claim_hook(root: &Root, event: &'static str, hook: &HookSnap) -> bool {
    let (claimed, unregistered) = root.events.claim(event, &hook.id);
    if unregistered {
        let meta = ListenerObservationMeta {
            id: hook.id.clone(),
            event,
            role: hook.kind.role(),
            scope: crate::observation::ScopeId::from_layer(
                &root.scope_membership,
                hook.scope.clone(),
            ),
            options: hook.options,
        };
        publish_listener_change(
            root,
            crate::observation::ListenerChange::Unregistered,
            &meta,
        );
    }
    claimed
}

impl Context {
    fn register_listener<E, L>(
        &self,
        listener: L,
        options: ListenerOptions,
    ) -> Result<ListenerRegistration, ListenerRegistrationError>
    where
        E: Event,
        L: Listener<E>,
    {
        let kind = super::listener::into_hook::<E, L>(listener, self.clone());
        let fiber = self.fiber().clone();
        let store = self.root.events.clone();
        let id = super::types::fresh_listener_registration_id();
        let meta = ListenerObservationMeta {
            id: id.clone(),
            event: E::NAME,
            role: kind.role(),
            scope: crate::observation::ScopeId::from_context(self),
            options,
        };
        let hook = Hook::new(self, kind, options, id.clone());
        let mut publish = super::store::HookPublish::<E> {
            store: store.clone(),
            hook: Some(hook),
            _event: std::marker::PhantomData,
            contract_mismatch: false,
        };
        let token = crate::gated::push_gated(
            &fiber,
            listener_cleanup(self.root.clone(), store.clone(), meta.clone()),
            &mut publish,
        )
        .map_err(|_| {
            if publish.contract_mismatch {
                ListenerRegistrationError::EventContractMismatch { event: E::NAME }
            } else {
                ListenerRegistrationError::InactiveContext
            }
        })?;
        publish_listener_change(
            &self.root,
            crate::observation::ListenerChange::Registered,
            &meta,
        );
        let remove_store = store.clone();
        let remove_root = self.root.clone();
        let remove_meta = meta.clone();
        Ok(ListenerRegistration {
            remove: Arc::new(move |registration| {
                let removed = remove_store.remove(E::NAME, registration);
                if removed {
                    publish_listener_change(
                        &remove_root,
                        crate::observation::ListenerChange::Unregistered,
                        &remove_meta,
                    );
                }
                removed
            }),
            id,
            cleanup: token,
            owner: Arc::downgrade(&fiber),
        })
    }

    /// Register one semantic listener with default options.
    pub fn on<E, L>(&self, listener: L) -> Result<ListenerRegistration, ListenerRegistrationError>
    where
        E: Event,
        L: Listener<E>,
    {
        self.register_listener::<E, L>(listener, ListenerOptions::default())
    }

    /// Register one semantic listener with explicit options.
    pub fn on_with<E, L>(
        &self,
        listener: L,
        options: ListenerOptions,
    ) -> Result<ListenerRegistration, ListenerRegistrationError>
    where
        E: Event,
        L: Listener<E>,
    {
        self.register_listener::<E, L>(listener, options)
    }

    fn preflight<E: Event>(
        &self,
        routing: &Routing,
        operation: EventOperation,
    ) -> Result<Vec<HookSnap>, DispatchError> {
        if let Routing::Scoped(scope) = routing
            && !scope.belongs_to(&self.root.scope_membership)
        {
            return Err(DispatchError::ForeignScope);
        }
        self.root.events.preflight::<E>(routing, operation)
    }

    async fn invoke_plain(
        callback: ErasedListener,
        payload: ErasedPayload,
    ) -> Result<CallbackValue, InvocationFailure> {
        let future = std::panic::catch_unwind(AssertUnwindSafe(|| callback(payload)))
            .map_err(|panic| InvocationFailure::panic(panic_message(panic)))?;
        AssertUnwindSafe(future)
            .catch_unwind()
            .await
            .map_err(|panic| InvocationFailure::panic(panic_message(panic)))?
    }

    async fn invoke_listener(
        callback: ErasedListener,
        payload: ErasedPayload,
        registration: ListenerRegistrationId,
    ) -> Result<CallbackValue, InvocationFailure> {
        Self::invoke_plain(callback, payload)
            .await
            .map_err(|failure| failure.correlate(registration))
    }

    async fn invoke_around(
        callback: ErasedAroundListener,
        payload: ErasedPayload,
        next: NextFn,
    ) -> Result<ErasedPayload, InvocationFailure> {
        let future = std::panic::catch_unwind(AssertUnwindSafe(|| callback(payload, next)))
            .map_err(|panic| InvocationFailure::panic(panic_message(panic)))?;
        AssertUnwindSafe(future)
            .catch_unwind()
            .await
            .map_err(|panic| InvocationFailure::panic(panic_message(panic)))?
    }

    /// Deliver an ordered notification using explicit routing.
    async fn emit_inner<E>(&self, routing: Routing, args: E::Args) -> Result<(), DispatchError>
    where
        E: Event,
        E::Args: Clone,
    {
        let hooks = self.preflight::<E>(&routing, EventOperation::Emit)?;
        for hook in hooks {
            if !claim_hook(&self.root, E::NAME, &hook) {
                continue;
            }
            let registration = hook.id;
            let HookKind::Plain { callback, .. } = hook.kind else {
                unreachable!("preflight excludes Around from emit")
            };
            Self::invoke_listener(callback, Box::new(args.clone()), registration)
                .await
                .map_err(DispatchError::Invocation)?;
        }
        Ok(())
    }

    /// Deliver a parallel notification using explicit routing.
    async fn emit_parallel_inner<E>(
        &self,
        routing: Routing,
        args: E::Args,
    ) -> Result<(), DispatchError>
    where
        E: Event,
        E::Args: Clone,
    {
        let hooks = self.preflight::<E>(&routing, EventOperation::EmitParallel)?;
        let claimed = hooks
            .into_iter()
            .filter(|hook| claim_hook(&self.root, E::NAME, hook))
            .collect::<Vec<_>>();

        let invocations = claimed.into_iter().map(|hook| {
            let registration = hook.id;
            let HookKind::Plain { callback, .. } = hook.kind else {
                unreachable!("preflight excludes Around from emit_parallel")
            };
            Self::invoke_listener(callback, Box::new(args.clone()), registration)
        });
        let failures = join_all(invocations)
            .await
            .into_iter()
            .filter_map(Result::err)
            .collect::<Vec<_>>();
        if failures.is_empty() {
            Ok(())
        } else {
            Err(DispatchError::Parallel(ParallelFailures::new(failures)))
        }
    }

    /// Query selected Observer/Responder registrations for the first answer.
    async fn query_inner<E>(
        &self,
        routing: Routing,
        args: E::Args,
    ) -> Result<QueryOutcome<E::Output>, DispatchError>
    where
        E: Event,
        E::Args: Clone,
    {
        let hooks = self.preflight::<E>(&routing, EventOperation::Query)?;
        for hook in hooks {
            if !claim_hook(&self.root, E::NAME, &hook) {
                continue;
            }
            let registration = hook.id;
            let HookKind::Plain { callback, role } = hook.kind else {
                unreachable!("preflight excludes Around from query")
            };
            let value = Self::invoke_listener(callback, Box::new(args.clone()), registration)
                .await
                .map_err(DispatchError::Invocation)?;
            match (role, value) {
                (super::ListenerRole::Observer, CallbackValue::Observer) => {}
                (super::ListenerRole::Responder, CallbackValue::Responder(None)) => {}
                (super::ListenerRole::Responder, CallbackValue::Responder(Some(output))) => {
                    let output = *output
                        .downcast::<E::Output>()
                        .expect("Event contract guarantees Responder output type");
                    return Ok(QueryOutcome::Answer(output));
                }
                _ => unreachable!("semantic adapter role determines callback value"),
            }
        }
        Ok(QueryOutcome::Miss)
    }

    /// Run an owned Mapper/Around waterfall using explicit routing.
    async fn waterfall_inner<E, F, Fut, Err>(
        &self,
        routing: Routing,
        args: E::Args,
        tail: F,
    ) -> Result<E::Output, DispatchError>
    where
        E: Event,
        F: FnOnce(E::Args) -> Fut + Send + 'static,
        Fut: Future<Output = Result<E::Output, Err>> + Send + 'static,
        Err: Error + 'static,
    {
        let hooks = self.preflight::<E>(&routing, EventOperation::Waterfall)?;
        let tail = NextFn(Box::new(move |payload| {
            let args = *payload
                .downcast::<E::Args>()
                .expect("Event contract guarantees waterfall tail Args type");
            let future = std::panic::catch_unwind(AssertUnwindSafe(|| tail(args)))
                .map_err(|panic| InvocationFailure::panic(panic_message(panic)));
            Box::pin(async move {
                let future = future?;
                let result = AssertUnwindSafe(future)
                    .catch_unwind()
                    .await
                    .map_err(|panic| InvocationFailure::panic(panic_message(panic)))?;
                match result {
                    Ok(output) => Ok(Box::new(output) as ErasedPayload),
                    Err(error) => {
                        let diagnostic =
                            std::panic::catch_unwind(AssertUnwindSafe(|| error.to_string()))
                                .map_err(|panic| InvocationFailure::panic(panic_message(panic)))?;
                        Err(InvocationFailure::returned(diagnostic))
                    }
                }
            })
        }));

        let mut next = tail;
        for hook in hooks.into_iter().rev() {
            let downstream = next;
            let root = self.root.clone();
            next = NextFn(Box::new(move |payload| {
                Box::pin(async move {
                    if !claim_hook(&root, E::NAME, &hook) {
                        return downstream.call(payload).await;
                    }
                    let registration = hook.id.clone();
                    match hook.kind {
                        HookKind::Plain { callback, role } => {
                            debug_assert_eq!(role, super::ListenerRole::Mapper);
                            match Context::invoke_plain(callback, payload)
                                .await
                                .map_err(|failure| failure.correlate(registration))?
                            {
                                CallbackValue::Mapper(mapped) => downstream.call(mapped).await,
                                _ => unreachable!("Mapper adapter returns a mapped payload"),
                            }
                        }
                        HookKind::Around(callback) => {
                            Context::invoke_around(callback, payload, downstream)
                                .await
                                .map_err(|failure| failure.correlate(registration))
                        }
                    }
                })
            }));
        }

        let output = next
            .call(Box::new(args))
            .await
            .map_err(DispatchError::Invocation)?;
        Ok(*output
            .downcast::<E::Output>()
            .expect("Event contract guarantees waterfall output type"))
    }

    /// Deliver an ordered notification using explicit routing.
    pub async fn emit<E>(&self, routing: Routing, args: E::Args) -> Result<(), DispatchError>
    where
        E: Event,
        E::Args: Clone,
    {
        let observed_routing = crate::observation::ObservationRouting::from_routing(&routing);
        let result = self.emit_inner::<E>(routing, args).await;
        self.root
            .observations
            .publish(crate::observation::RuntimeObservation::DispatchCompleted {
                operation: EventOperation::Emit,
                event: E::NAME,
                routing: observed_routing,
                outcome: if result.is_ok() {
                    super::DispatchOutcomeKind::Completed
                } else {
                    super::DispatchOutcomeKind::Failed
                },
            });
        result
    }

    /// Deliver a parallel notification using explicit routing.
    pub async fn emit_parallel<E>(
        &self,
        routing: Routing,
        args: E::Args,
    ) -> Result<(), DispatchError>
    where
        E: Event,
        E::Args: Clone,
    {
        let observed_routing = crate::observation::ObservationRouting::from_routing(&routing);
        let result = self.emit_parallel_inner::<E>(routing, args).await;
        self.root
            .observations
            .publish(crate::observation::RuntimeObservation::DispatchCompleted {
                operation: EventOperation::EmitParallel,
                event: E::NAME,
                routing: observed_routing,
                outcome: if result.is_ok() {
                    super::DispatchOutcomeKind::Completed
                } else {
                    super::DispatchOutcomeKind::Failed
                },
            });
        result
    }

    /// Query selected Observer/Responder registrations for the first answer.
    pub async fn query<E>(
        &self,
        routing: Routing,
        args: E::Args,
    ) -> Result<QueryOutcome<E::Output>, DispatchError>
    where
        E: Event,
        E::Args: Clone,
    {
        let observed_routing = crate::observation::ObservationRouting::from_routing(&routing);
        let result = self.query_inner::<E>(routing, args).await;
        let outcome = match &result {
            Ok(QueryOutcome::Answer(_)) => super::DispatchOutcomeKind::Answered,
            Ok(QueryOutcome::Miss) => super::DispatchOutcomeKind::Missed,
            Err(_) => super::DispatchOutcomeKind::Failed,
        };
        self.root
            .observations
            .publish(crate::observation::RuntimeObservation::DispatchCompleted {
                operation: EventOperation::Query,
                event: E::NAME,
                routing: observed_routing,
                outcome,
            });
        result
    }

    /// Run an owned Mapper/Around waterfall using explicit routing.
    pub async fn waterfall<E, F, Fut, Err>(
        &self,
        routing: Routing,
        args: E::Args,
        tail: F,
    ) -> Result<E::Output, DispatchError>
    where
        E: Event,
        F: FnOnce(E::Args) -> Fut + Send + 'static,
        Fut: Future<Output = Result<E::Output, Err>> + Send + 'static,
        Err: Error + 'static,
    {
        let observed_routing = crate::observation::ObservationRouting::from_routing(&routing);
        let result = self
            .waterfall_inner::<E, F, Fut, Err>(routing, args, tail)
            .await;
        self.root
            .observations
            .publish(crate::observation::RuntimeObservation::DispatchCompleted {
                operation: EventOperation::Waterfall,
                event: E::NAME,
                routing: observed_routing,
                outcome: if result.is_ok() {
                    super::DispatchOutcomeKind::Completed
                } else {
                    super::DispatchOutcomeKind::Failed
                },
            });
        result
    }

    /// Run a waterfall whose mandatory tail performs one query with the same routing.
    ///
    /// The resolver receives the complete query result, including dispatch failure,
    /// and its own failure is treated as framework-tail failure by the surrounding
    /// waterfall.
    pub async fn waterfall_query<E, Q, R, Fut, Err>(
        &self,
        routing: Routing,
        args: E::Args,
        resolver: R,
    ) -> Result<E::Output, DispatchError>
    where
        E: Event,
        Q: Event<Args = E::Args, Output = E::Output>,
        E::Args: Clone,
        R: FnOnce(Result<QueryOutcome<E::Output>, DispatchError>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<E::Output, Err>> + Send + 'static,
        Err: Error + 'static,
    {
        let query_ctx = self.clone();
        let query_routing = routing.clone();
        self.waterfall::<E, _, _, Err>(routing, args, move |query_args| async move {
            let query = query_ctx.query::<Q>(query_routing, query_args).await;
            resolver(query).await
        })
        .await
    }
}

fn listener_cleanup(
    root: Arc<Root>,
    store: Arc<super::store::EventStore>,
    meta: ListenerObservationMeta,
) -> crate::effect::Cleanup {
    crate::effect::sync_cleanup(move || {
        if store.remove(meta.event, &meta.id) {
            publish_listener_change(
                &root,
                crate::observation::ListenerChange::Unregistered,
                &meta,
            );
        }
    })
}
