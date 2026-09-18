//! Standard host capabilities exposed to Cordis Components.

mod configuration;
mod diagnostics;
mod events;

use wasmtime::component::{HasData, Linker};

use crate::HostState;

pub use events::{ComponentEvent, HostEvent};

/// Shared Wasmtime projection for all standard capability interfaces.
///
/// A Component world has one generated linker entrypoint. Individual
/// capabilities implement their generated traits against this projection while
/// remaining isolated in their own modules.
pub(crate) struct HostCapabilities;

impl HasData for HostCapabilities {
    type Data<'a> = &'a mut HostState;
}

/// Add every standard capability import to a component linker.
pub(crate) fn add_to_linker(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    crate::bindings::CordisPlugin::add_to_linker::<_, HostCapabilities>(linker, |state| state)
}
