//! Guest diagnostics used by the native `logging_exporters` host scenario.
//!
//! Exporter installation and filtering remain host authority. The guest only
//! emits records through the logger channel assigned to its Fiber.

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

struct LoggingProbe;

impl Guest for LoggingProbe {
    async fn activate() -> Result<(), LifecycleError> {
        emit(Level::Debug, "guest debug diagnostic".into()).await;
        emit(Level::Info, "guest info diagnostic".into()).await;
        emit(Level::Warn, "guest warning diagnostic".into()).await;
        emit(Level::Error, "guest error diagnostic".into()).await;
        Ok(())
    }

    async fn dispose() -> Result<(), LifecycleError> {
        emit(Level::Info, "guest diagnostics disposed".into()).await;
        Ok(())
    }
}

impl ManifestGuest for LoggingProbe {
    async fn describe() -> Descriptor {
        Descriptor {
            name: "logging-exporters".into(),
            standard_capabilities: vec![StandardCapability::Diagnostics],
            subscribed_events: vec![],
        }
    }
}

impl EventHandlerGuest for LoggingProbe {
    async fn handle(_: Event) -> Result<(), cordis::plugin::events::EventError> {
        Ok(())
    }
}

export!(LoggingProbe);
