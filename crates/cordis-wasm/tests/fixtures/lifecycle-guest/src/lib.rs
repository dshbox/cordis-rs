wit_bindgen::generate!({
    path: "../../../../../wit",
    world: "cordis-plugin",
    async: true,
});

use exports::cordis::plugin::lifecycle::{Guest, LifecycleError};
use exports::cordis::plugin::manifest::{Descriptor, Guest as ManifestGuest, StandardCapability};
use exports::cordis::plugin::event_handler::Guest as EventHandlerGuest;
use cordis::plugin::configuration::get;
use cordis::plugin::diagnostics::{Level, emit};
use cordis::plugin::events::{Event, emit as emit_event};

struct Component;

impl Guest for Component {
    async fn activate() -> Result<(), LifecycleError> {
        let configuration = String::from_utf8(get().await).expect("fixture configuration is UTF-8");
        emit_event(Event {
            name: "fixture/activated".into(),
            payload: configuration.as_bytes().to_vec(),
        })
        .await
        .expect("host accepts fixture event");
        emit(Level::Info, format!("guest configuration: {configuration}")).await;
        emit(Level::Info, "guest lifecycle activated".into()).await;
        Ok(())
    }

    async fn dispose() -> Result<(), LifecycleError> {
        emit(Level::Info, "guest lifecycle disposed".into()).await;
        Ok(())
    }
}

impl ManifestGuest for Component {
    async fn describe() -> Descriptor {
        emit(Level::Debug, "guest manifest read".into()).await;
        Descriptor {
            name: "lifecycle-guest".into(),
            standard_capabilities: vec![
                StandardCapability::Configuration,
                StandardCapability::Diagnostics,
            ],
            subscribed_events: vec!["fixture/inbound".into()],
        }
    }
}

impl EventHandlerGuest for Component {
    async fn handle(event: Event) -> Result<(), cordis::plugin::events::EventError> {
        emit(Level::Info, format!("guest received: {}", event.name)).await;
        Ok(())
    }
}

export!(Component);
