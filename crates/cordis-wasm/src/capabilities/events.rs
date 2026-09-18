//! Guest-originated Component event capability.

use std::future::Future;

use cordis_core::{Event, Routing};
use wasmtime::component::Accessor;

use crate::HostState;
use crate::bindings;

/// An opaque event emitted by a guest Component.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentEvent {
    name: String,
    payload: Vec<u8>,
}

/// An event a native Cordis host delivers to subscribed guest Components.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostEvent {
    pub(crate) name: String,
    pub(crate) payload: Vec<u8>,
}

impl HostEvent {
    /// Construct one event for guest Components that declared this name.
    pub fn new(name: impl Into<String>, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            name: name.into(),
            payload: payload.into(),
        }
    }
}

impl Event for HostEvent {
    const NAME: &'static str = "cordis/host-event";
    type Args = HostEvent;
    type Output = ();
}

impl ComponentEvent {
    /// Construct one event for delivery to guest-event observers.
    pub fn new(name: impl Into<String>, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            name: name.into(),
            payload: payload.into(),
        }
    }
    /// The guest-defined event name.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// The guest-defined opaque payload.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}

impl Event for ComponentEvent {
    const NAME: &'static str = "cordis/component-event";
    type Args = ComponentEvent;
    type Output = ();
}

impl bindings::cordis::plugin::events::Host for HostState {}

impl bindings::cordis::plugin::events::HostWithStore<HostState>
    for crate::capabilities::HostCapabilities
{
    fn emit(
        host: &Accessor<HostState, crate::capabilities::HostCapabilities>,
        event: bindings::cordis::plugin::events::Event,
    ) -> impl Future<Output = Result<(), bindings::cordis::plugin::events::EventError>> + Send {
        let context = host.with(|mut access| access.get().context.clone());
        let event = ComponentEvent::new(event.name, event.payload);
        async move {
            context
                .emit::<ComponentEvent>(Routing::Unscoped, event)
                .await
                .map(|_| ())
                .map_err(|error| bindings::cordis::plugin::events::EventError {
                    message: error.to_string(),
                })
        }
    }
}
