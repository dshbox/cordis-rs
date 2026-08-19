//! Cordis is a context-based plugin framework with scoped dependency injection,
//! lifecycle-owned effects, events, configuration interception, and structured
//! logging.
//!
//! This crate is a Rust port of `@deepseek-ai/cordis` 4.x.  The JavaScript
//! implementation relies heavily on proxies, prototype chains, callable
//! objects, and decorators.  The Rust API keeps the same runtime model while
//! replacing those language features with explicit, typed methods.
//!
//! Naming note: the package is published as `cordis-rs`, but the library
//! name and import path are `cordis` (matching upstream), and the sources
//! live under `crates/cordis` in the repository.
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
//! fiber.try_wait().unwrap();
//! assert_eq!(counter.load(Ordering::SeqCst), 1);
//! fiber.dispose().unwrap();
//! ```
//!
//! # Naming vocabulary
//!
//! The API keeps upstream Cordis verbs (`emit`, `bail`, `waterfall`,
//! `serial`, `parallel`, `on`, `once`, `provide`, `get`, `set`, `inject`,
//! `isolate`, `intercept`, `effect`, `plugin`, `accessor`, `require`,
//! `notify`) and renames them only when a Rust convention conflicts
//! strongly (e.g. `Event::call_next` instead of upstream's `next()`, which
//! collides with [`Iterator::next`]). The
//! suffix/prefix conventions:
//!
//! - `*_value` — type-erased (`Value`/`Config`) variant; `resolved_*` —
//!   evaluated (as opposed to static) result; `with_*` — builder or
//!   "with extra parameter" variant; `*Service` — context-bound service
//!   facade.
//! - `is_`/`has_` — predicates; `assert_` — panics (fallible checks are
//!   `ensure_`, e.g. [`Fiber::ensure_active`]); `*_unchecked` is reserved
//!   for genuinely `unsafe` APIs (this crate has none).
//! - `as_` — cheap borrow; `to_` — expensive copy; `into_` — ownership
//!   transfer.
//! - `_async` suffixes mark genuinely suspending functions. The one
//!   exception is [`Fiber::dispose_async`], kept for upstream parity with
//!   `disposeAsync`: it is a synchronous pass-through that never yields.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Version of the cordis-rs core this binary was compiled against.
///
/// Dynamic-library plugins (`cordis-loader`'s `dynamic` feature) embed this
/// in their build fingerprint so a library built against a different core
/// version is rejected instead of producing undefined behavior.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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
    ANSI16_PALETTE, ANSI256_PALETTE, Exporter, ExporterConfig, FormatterFn, LogArg, LogKind,
    Logger, LoggerIntercept, LoggerLevel, LoggerService, Message, color_code, default_format,
};
pub use reflect::{Accessor, Property, ReflectService, ServiceInfo};
pub use registry::{
    Inject, IntoPlugin, Plugin, PluginHandle, PluginKey, PluginOutput, RegistryService,
    RuntimeInfo, plugin_async, plugin_sync,
};
pub use service::{Service, service_async, service_sync};
pub use value::{Config, Value};
