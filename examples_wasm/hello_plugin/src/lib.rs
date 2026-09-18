//! The guest half of the native `hello_plugin` example.
//!
//! Cordis owns listener registration and teardown. This Component declares
//! `ping`, receives delivery only while its apply generation is live, and
//! emits the observable behavior through host-owned diagnostics.

wit_bindgen::generate!({
    path: "../../wit",
    world: "cordis-plugin",
    async: true,
});

use cordis::plugin::diagnostics::{Level, emit};
use cordis::plugin::events::Event;
use exports::cordis::plugin::event_handler::Guest as EventHandlerGuest;
use exports::cordis::plugin::lifecycle::{Guest, LifecycleError};
use exports::cordis::plugin::manifest::{Descriptor, Guest as ManifestGuest, StandardCapability};

struct Echo;

impl Guest for Echo {
    async fn activate() -> Result<(), LifecycleError> {
        emit(Level::Debug, "hello plugin activated".into()).await;
        Ok(())
    }

    async fn dispose() -> Result<(), LifecycleError> {
        emit(Level::Debug, "hello plugin disposed".into()).await;
        Ok(())
    }
}

impl ManifestGuest for Echo {
    async fn describe() -> Descriptor {
        Descriptor {
            name: "hello-plugin".into(),
            standard_capabilities: vec![StandardCapability::Diagnostics],
            subscribed_events: vec!["ping".into()],
        }
    }
}

impl EventHandlerGuest for Echo {
    async fn handle(event: Event) -> Result<(), cordis::plugin::events::EventError> {
        if event.name != "ping" {
            return Ok(());
        }
        let name = core::str::from_utf8(&event.payload).map_err(|error| {
            cordis::plugin::events::EventError {
                message: format!("ping payload must be UTF-8: {error}"),
            }
        })?;
        emit(Level::Info, format!("hello, {name}")).await;
        Ok(())
    }
}

export!(Echo);
