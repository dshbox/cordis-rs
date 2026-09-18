//! Host-owned diagnostics capability.

use std::future::Future;

use cordis_core::Level;
use wasmtime::component::Accessor;

use crate::HostState;
use crate::bindings;

impl bindings::cordis::plugin::diagnostics::Host for HostState {}

impl bindings::cordis::plugin::diagnostics::HostWithStore<HostState>
    for crate::capabilities::HostCapabilities
{
    fn emit(
        host: &Accessor<HostState, crate::capabilities::HostCapabilities>,
        level: bindings::cordis::plugin::diagnostics::Level,
        message: String,
    ) -> impl Future<Output = ()> + Send {
        let logger = host.with(|mut access| access.get().logger.clone());
        async move {
            let level = match level {
                bindings::cordis::plugin::diagnostics::Level::Debug => Level::Debug,
                bindings::cordis::plugin::diagnostics::Level::Info => Level::Info,
                bindings::cordis::plugin::diagnostics::Level::Warn => Level::Warn,
                bindings::cordis::plugin::diagnostics::Level::Error => Level::Error,
            };
            logger.log(level, message);
        }
    }
}
