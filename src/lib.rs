//! Cordis is a context-based plugin framework with scoped dependency injection,
//! lifecycle-owned effects, events, configuration interception, and structured
//! logging.
//!
//! This crate is a Rust port of `@deepseek-ai/cordis` 4.x.  The JavaScript
//! implementation relies heavily on proxies, prototype chains, callable
//! objects, and decorators.  The Rust API keeps the same runtime model while
//! replacing those language features with explicit, typed methods.
//!
//! # Quick start
//!
//! ```
//! use cordis::{plugin_sync, Context, Inject, PluginOutput};
//! use std::sync::atomic::{AtomicUsize, Ordering};
//! use std::sync::Arc;
//!
//! let root = Context::new();
//! let counter = Arc::new(AtomicUsize::new(0));
//! let _counter_effect = root.provide_arc("counter", counter.clone()).unwrap();
//!
//! let greeter = plugin_sync::<(), _>("greeter", Inject::new(["counter"]),
//!     |ctx, _config| {
//!         let counter = ctx.require::<AtomicUsize>("counter")?;
//!         counter.fetch_add(1, Ordering::SeqCst);
//!         Ok(PluginOutput::default())
//!     });
//!
//! let fiber = root.plugin(greeter, ());
//! fiber.wait().unwrap();
//! assert_eq!(counter.load(Ordering::SeqCst), 1);
//! fiber.dispose().unwrap();
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod context;
pub mod effect;
pub mod error;
pub mod events;
pub mod fiber;
pub mod logger;
pub mod reflect;
pub mod registry;
pub mod service;
pub mod utils;
pub mod value;

pub use context::{Context, ContextMeta, Isolation};
pub use effect::{AsyncDisposer, EffectHandle, EffectMeta};
pub use error::{CordisError, ErrorCode, Result, ValidationError, ValidationIssue};
pub use events::{
    DispatchMode, Event, EventOptions, EventResult, EventValue, EventsService, is_bailed,
};
pub use fiber::{Fiber, FiberState};
pub use logger::{
    C16, C256, Exporter, ExporterConfig, FormatterFn, LogArg, Logger, LoggerIntercept, LoggerLevel,
    LoggerService, LoggerType, Message, color_code, default_format,
};
pub use reflect::{Accessor, Property, ReflectService, ServiceInfo};
pub use registry::{
    Inject, IntoPlugin, Plugin, PluginHandle, PluginKey, PluginOutput, RegistryService,
    RuntimeInfo, plugin_async, plugin_sync,
};
pub use service::{Service, service_async, service_sync};
pub use value::{Config, Value};
