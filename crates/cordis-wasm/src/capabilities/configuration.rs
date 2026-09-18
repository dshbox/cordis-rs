//! Immutable per-generation configuration capability.

use std::future::Future;

use wasmtime::component::Accessor;

use crate::HostState;
use crate::bindings;

impl bindings::cordis::plugin::configuration::Host for HostState {}

impl bindings::cordis::plugin::configuration::HostWithStore<HostState>
    for crate::capabilities::HostCapabilities
{
    fn get(
        host: &Accessor<HostState, crate::capabilities::HostCapabilities>,
    ) -> impl Future<Output = Vec<u8>> + Send {
        let configuration = host.with(|mut access| access.get().configuration.clone());
        async move { configuration.to_vec() }
    }
}
