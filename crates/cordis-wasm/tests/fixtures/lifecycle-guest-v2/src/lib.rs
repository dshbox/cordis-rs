wit_bindgen::generate!({ path: "../../../../../wit", world: "cordis-plugin", async: true });

use cordis::plugin::diagnostics::{emit, Level};
use cordis::plugin::events::Event;
use exports::cordis::plugin::event_handler::Guest as EventHandlerGuest;
use exports::cordis::plugin::lifecycle::{Guest, LifecycleError};
use exports::cordis::plugin::manifest::{Descriptor, Guest as ManifestGuest};

struct Component;

impl Guest for Component {
    async fn activate() -> Result<(), LifecycleError> {
        emit(Level::Info, "v2 activate".into()).await;
        Ok(())
    }

    async fn dispose() -> Result<(), LifecycleError> {
        emit(Level::Info, "v2 dispose".into()).await;
        Ok(())
    }
}

impl ManifestGuest for Component {
    async fn describe() -> Descriptor {
        Descriptor {
            name: "lifecycle-guest-v2".into(),
            standard_capabilities: vec![],
            subscribed_events: vec![],
        }
    }
}

impl EventHandlerGuest for Component {
    async fn handle(_: Event) -> Result<(), cordis::plugin::events::EventError> {
        Ok(())
    }
}

export!(Component);
