//! Names of the events the loader emits on the root context's event bus.
//!
//! All listeners receive the affected [`Entry`](crate::Entry) as the first
//! argument; `config-update` additionally carries the new
//! [`Node`](crate::Node) config as the second. Listener failures are
//! recorded through [`Loader::last_error`](crate::Loader::last_error) and
//! never abort the state machine — keep listeners light, they run inside
//! loader transitions.

/// Emitted after an entry's fiber was created and mapped.
pub const ENTRY_INIT: &str = "loader/entry-init";

/// Emitted before a config-only patch is applied to a live fiber.
pub const BEFORE_PATCH: &str = "loader/before-patch";

/// Emitted after a config-only patch was applied.
pub const AFTER_PATCH: &str = "loader/after-patch";

/// Emitted after a self-disposed plugin was persisted as `disabled: true`.
pub const PARTIAL_DISPOSE: &str = "loader/partial-dispose";

/// Emitted after [`update_config`](crate::Loader::update_config) changed
/// and persisted an entry's config.
pub const CONFIG_UPDATE: &str = "loader/config-update";
