//! Dynamic-library plugins (behind the `dynamic` feature).
//!
//! Rust has no stable ABI, so a `.so`/`.dylib`/`.dll` and the process
//! loading it are only compatible when they were produced by the *same*
//! toolchain, target, panic strategy, and `cordis-rs` core version. This
//! module enforces exactly that through a build fingerprint, then lets the
//! two sides exchange a [`Plugin`] trait object directly.
//!
//! A plugin library is a `cdylib` crate that implements [`Plugin`] and ends
//! with [`export_plugin!`]:
//!
//! ```rust,ignore
//! // greeter/src/lib.rs — a crate with `crate-type = ["cdylib"]`
//! use cordis::{plugin_sync, Inject, PluginOutput};
//!
//! pub struct Greeter;
//!
//! cordis_loader::dynamic::export_plugin!(plugin_sync::<(), _>(
//!     "greeter",
//!     Inject::default(),
//!     |_ctx, _config| Ok(PluginOutput::none()),
//! ));
//! ```
//!
//! The loading side adds one or more plugin directories to a
//! [`PluginRegistry`](crate::PluginRegistry); entries whose `name` matches
//! the library file (`lib<name>.so` / `lib<name>.dylib` / `<name>.dll`)
//! resolve to a fresh instance of that plugin.
//!
//! # Safety model
//!
//! - **Fingerprint gate.** Before any Rust value crosses the boundary, the
//!   library must report this loader's exact [`fingerprint`]: ABI version,
//!   `cordis-rs` core version, rustc release *and* commit hash, target
//!   triple, and panic strategy. Any mismatch rejects the library — a
//!   same-version-different-rustc build is just as incompatible as a
//!   different cordis version, because vtable layouts differ.
//! - **Panic containment (plugin side).** A `cdylib` statically links its
//!   own copy of std, so a panic raised by plugin code is a *foreign*
//!   exception in the loading process — unwinding it into the loader, or
//!   trying to catch it there, aborts the process. All containment
//!   therefore happens inside the library: [`export_plugin!`] wraps the
//!   plugin in a guard that turns panics from `name`/`inject`/
//!   `validate_config`/`apply` (poll-time included) into fallback values
//!   and [`cordis::CordisError`]s, and the `extern "C"` entry points wrap
//!   construction in [`std::panic::catch_unwind`] because unwinding out
//!   of an `extern "C"` function is undefined behavior.
//! - **No in-process unload.** The library handle is intentionally leaked
//!   after a successful load: instances, vtables, and thread-local
//!   destructors may reference the mapping forever, so `dlclose` can never
//!   be proven safe. Unloading happens only at process exit — which is how
//!   HMR works here anyway: `cordis-cli` watches the plugin directories and
//!   restarts the whole worker (exit code 51), and the fresh process loads
//!   the new build. Replacing a `.so` in place is therefore *not* picked up
//!   by a running worker.
//! - **Thin pointers.** `Box<dyn Plugin>` is fat; [`export_plugin!`]
//!   double-boxes it (`Box<Box<dyn Plugin>>`) so the `*mut c_void` round
//!   trip through the C symbol is lossless.

use cordis::{ErrorCode, PluginHandle, VERSION};
use cordis_include::resolver::unknown_plugin;
use libloading::{Library, Symbol};
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

// The plugin-authoring prelude: re-exported so a plugin crate can depend
// on `cordis-loader` alone (these also serve as this module's imports).
pub use crate::export_plugin;
pub use cordis::utils::BoxFuture;
pub use cordis::{Config, Context, Inject, Plugin, PluginOutput, Result};

/// Symbol a dynamic library exports to negotiate the export protocol.
const ABI_SYMBOL: &[u8] = b"cordis_plugin_abi\0";
/// Symbol a dynamic library exports to report its build fingerprint.
const FINGERPRINT_SYMBOL: &[u8] = b"cordis_plugin_fingerprint\0";
/// Symbol a dynamic library exports to create one plugin instance.
const CREATE_SYMBOL: &[u8] = b"cordis_plugin_create\0";

/// Version of the dynamic-plugin export protocol (symbols, fingerprint
/// format, double-boxing contract). Bump whenever the wire format changes.
pub const DYNAMIC_ABI: u32 = 1;

/// The signature of `cordis_plugin_create`.
type CreateFn = unsafe extern "C" fn() -> *mut c_void;

/// This build's dynamic-plugin compatibility fingerprint.
///
/// Two sides can exchange a `dyn Plugin` only when every ingredient
/// matches, so the string pins the export protocol version, the
/// [`cordis`] core version (trait/vtable source of truth), the exact rustc
/// release *and* commit hash, the target triple, and the panic strategy —
/// `panic=abort` versus `unwind` changes both unwinding and codegen.
///
/// The ingredients come from this crate's build script and are baked into
/// the dependent's compilation: evaluating them inside a plugin build and
/// inside the loading application yields the respective build's
/// fingerprint, and equality is the compatibility check.
pub fn fingerprint() -> &'static str {
    static FINGERPRINT: OnceLock<String> = OnceLock::new();
    FINGERPRINT.get_or_init(|| {
        format!(
            "cordis-dynamic/{DYNAMIC_ABI} cordis-rs/{VERSION} \
             rustc/{}-{} target/{} panic/{}",
            env!("CORDIS_RUSTC_RELEASE"),
            env!("CORDIS_RUSTC_COMMIT_HASH"),
            env!("CORDIS_BUILD_TARGET"),
            env!("CORDIS_BUILD_PANIC"),
        )
    })
}

/// Backing for the `cordis_plugin_fingerprint` export; macro-internal.
///
/// Returns a pointer to a NUL-terminated copy of [`fingerprint`] that is
/// initialized once and lives for the remainder of the process.
pub fn plugin_fingerprint_cstr() -> *const c_char {
    static FINGERPRINT_C: OnceLock<CString> = OnceLock::new();
    FINGERPRINT_C
        .get_or_init(|| {
            // The fingerprint's ingredients cannot contain interior NULs.
            CString::new(fingerprint()).expect("fingerprint without interior NUL")
        })
        .as_ptr()
}

/// Create one plugin instance for the `cordis_plugin_create` export;
/// macro-internal.
///
/// The plugin is wrapped in [`PanicGuard`] — a cdylib links its own copy
/// of std, so panics must be caught on this side of the boundary — and
/// double-boxed: `Box<dyn Plugin>` is a fat pointer that cannot round-trip
/// through `*mut c_void`, while the outer `Box<Box<dyn Plugin>>` is thin.
/// Panics are contained because unwinding out of an `extern "C"` function
/// is undefined behavior; on panic the function returns null, which the
/// loading side reports as an error.
pub fn create_boxed_plugin(make: impl FnOnce() -> Box<dyn Plugin>) -> *mut c_void {
    catch_unwind(AssertUnwindSafe(|| {
        let plugin: Box<dyn Plugin> = make();
        let guarded: Box<dyn Plugin> = Box::new(PanicGuard { inner: plugin });
        let boxed: Box<Box<dyn Plugin>> = Box::new(guarded);
        Box::into_raw(boxed) as *mut c_void
    }))
    .unwrap_or(std::ptr::null_mut())
}

/// Name reported when a plugin panics in `name()`.
const PANICKED_NAME: &str = "(plugin panicked in name())";

/// An empty inject declaration for plugins that panic in `inject()`.
fn empty_inject() -> &'static Inject {
    static EMPTY: OnceLock<Inject> = OnceLock::new();
    EMPTY.get_or_init(|| Inject::new(Vec::<String>::new()))
}

/// Wraps a plugin so panics in its callbacks stay inside the dynamic
/// library that raised them ([`export_plugin!`] installs it).
///
/// A `cdylib` statically links its own copy of std: a panic raised by the
/// plugin is a *foreign* exception in the loading process, and catching it
/// there aborts ("Rust cannot catch foreign exceptions"). All containment
/// therefore happens on the plugin side — this type and its
/// [`std::panic::catch_unwind`] calls are compiled into the library and
/// share std with the panicking code. Panics surface as a fallback name,
/// an empty inject declaration, or a [`cordis::CordisError`].
pub struct PanicGuard {
    inner: Box<dyn Plugin>,
}

impl Plugin for PanicGuard {
    fn name(&self) -> &str {
        catch_unwind(AssertUnwindSafe(|| self.inner.name())).unwrap_or(PANICKED_NAME)
    }

    fn inject(&self) -> &Inject {
        catch_unwind(AssertUnwindSafe(|| self.inner.inject())).unwrap_or_else(|_| empty_inject())
    }

    fn validate_config(&self, config: Config) -> Result<Config> {
        catch_unwind(AssertUnwindSafe(|| self.inner.validate_config(config)))
            .unwrap_or_else(|panic| Err(guard_panic("validate_config()", panic)))
    }

    fn apply(&self, ctx: Context, config: Config) -> BoxFuture<Result<PluginOutput>> {
        // The synchronous call producing the future is guarded here; the
        // future itself is wrapped by GuardedFuture so poll-time panics
        // are contained as well.
        let inner = catch_unwind(AssertUnwindSafe(|| self.inner.apply(ctx, config)))
            .unwrap_or_else(|panic| {
                let error = guard_panic("apply()", panic);
                Box::pin(async move { Err(error) })
            });
        Box::pin(GuardedFuture { inner })
    }
}

/// The [`PanicGuard`] future: polls the plugin's apply future with
/// catch_unwind so panics raised during polling become apply errors.
struct GuardedFuture {
    inner: BoxFuture<Result<PluginOutput>>,
}

impl std::future::Future for GuardedFuture {
    type Output = Result<PluginOutput>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        match catch_unwind(AssertUnwindSafe(|| self.inner.as_mut().poll(cx))) {
            Ok(poll) => poll,
            Err(panic) => std::task::Poll::Ready(Err(guard_panic("apply()", panic))),
        }
    }
}

/// A panic payload as a [`cordis::CordisError`].
fn guard_panic(operation: &str, panic: Box<dyn std::any::Any + Send>) -> cordis::CordisError {
    let message = if let Some(text) = panic.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = panic.downcast_ref::<String>() {
        text.clone()
    } else {
        "non-string panic payload".to_owned()
    };
    cordis::CordisError::with_message(
        ErrorCode::Plugin,
        format!("dynamic plugin panicked in {operation}: {message}"),
    )
}

/// Exports a plugin implementation through the cordis dynamic-plugin
/// protocol so `cordis-loader`'s `dynamic` feature can load it.
///
/// Invoke once, at crate root of a `crate-type = ["cdylib"]` crate that
/// depends on `cordis-loader` with the `dynamic` feature; `$make` is
/// evaluated for every [`cordis::PluginHandle`] the loader resolves, so
/// each entry gets its own instance. The file name decides the plugin
/// name: `libgreeter.so` / `libgreeter.dylib` / `greeter.dll` resolve as
/// `greeter`.
///
/// ```rust,ignore
/// cordis_loader::dynamic::export_plugin!(MyPlugin::new());
/// ```
#[macro_export]
macro_rules! export_plugin {
    ($make:expr) => {
        /// cordis dynamic-plugin export protocol version.
        #[unsafe(no_mangle)]
        pub extern "C" fn cordis_plugin_abi() -> u32 {
            $crate::dynamic::DYNAMIC_ABI
        }

        /// NUL-terminated build fingerprint of this plugin library.
        #[unsafe(no_mangle)]
        pub extern "C" fn cordis_plugin_fingerprint() -> *const ::std::ffi::c_char {
            $crate::dynamic::plugin_fingerprint_cstr()
        }

        /// One freshly created, double-boxed plugin instance (null on panic).
        #[unsafe(no_mangle)]
        pub extern "C" fn cordis_plugin_create() -> *mut ::std::ffi::c_void {
            $crate::dynamic::create_boxed_plugin(|| Box::new($make))
        }
    };
}

/// A successfully loaded plugin library: the create entry point plus the
/// file it came from. The library handle itself is intentionally leaked
/// (see the [module docs](self)).
#[derive(Clone)]
struct LoadedPlugin {
    create: CreateFn,
    path: PathBuf,
}

/// Resolves entry names to plugins compiled as dynamic libraries.
///
/// Each directory is searched for the platform's `cdylib` naming scheme
/// (`lib<name>.so` on Linux, `lib<name>.dylib` on macOS, `<name>.dll` on
/// Windows). A library is opened and fingerprint-checked once; every
/// resolve then asks it for a fresh instance wrapped in a new
/// [`PluginHandle`], mirroring the static registry's one-instance-per-entry
/// semantics.
///
/// Clones share the loaded-library cache, so a registry and its clones
/// never open the same file twice.
pub struct DynamicPluginResolver {
    dirs: Arc<Vec<PathBuf>>,
    loaded: Arc<Mutex<HashMap<String, LoadedPlugin>>>,
}

impl DynamicPluginResolver {
    /// A resolver searching `dirs` (earlier directories win).
    pub fn new<I, D>(dirs: I) -> Self
    where
        I: IntoIterator<Item = D>,
        D: Into<PathBuf>,
    {
        Self {
            dirs: Arc::new(dirs.into_iter().map(Into::into).collect()),
            loaded: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// The directories searched, in order.
    pub fn dirs(&self) -> &[PathBuf] {
        &self.dirs
    }

    /// The first existing library file that would satisfy `name`, without
    /// loading it.
    pub fn plugin_path(&self, name: &str) -> Option<PathBuf> {
        self.dirs
            .iter()
            .flat_map(|dir| {
                library_file_names(name)
                    .into_iter()
                    .map(move |file| dir.join(file))
            })
            .find(|path| path.is_file())
    }

    fn loaded_plugin(&self, name: &str) -> Result<LoadedPlugin> {
        // The lock is held across loading: dlopen and the symbol checks are
        // fast, and holding it makes check-then-load atomic.
        let mut loaded = crate::lock(&self.loaded);
        if let Some(plugin) = loaded.get(name) {
            return Ok(plugin.clone());
        }
        let Some(path) = self.plugin_path(name) else {
            return Err(unknown_plugin(name));
        };
        let plugin = load_library(name, &path)?;
        loaded.insert(name.to_owned(), plugin.clone());
        Ok(plugin)
    }
}

impl Clone for DynamicPluginResolver {
    fn clone(&self) -> Self {
        Self {
            dirs: Arc::clone(&self.dirs),
            loaded: Arc::clone(&self.loaded),
        }
    }
}

impl cordis_include::PluginResolver for DynamicPluginResolver {
    fn resolve(&self, name: &str) -> Result<PluginHandle> {
        validate_name(name)?;
        let loaded = self.loaded_plugin(name)?;
        let plugin = create_instance(name, &loaded)?;
        Ok(PluginHandle::new(plugin))
    }
}

/// Reject names that could escape the plugin directories.
fn validate_name(name: &str) -> Result<()> {
    let invalid = name.is_empty()
        || name == "."
        || name == ".."
        || name.chars().any(|c| matches!(c, '/' | '\\' | '\0' | ':'));
    if invalid {
        return Err(cordis::CordisError::with_message(
            ErrorCode::Plugin,
            format!("invalid dynamic plugin name `{name}`"),
        ));
    }
    Ok(())
}

/// Platform `cdylib` file names for a plugin name.
fn library_file_names(name: &str) -> Vec<String> {
    if cfg!(target_os = "macos") {
        vec![format!("lib{name}.dylib"), format!("{name}.dylib")]
    } else if cfg!(target_os = "windows") {
        vec![format!("{name}.dll"), format!("lib{name}.dll")]
    } else {
        vec![format!("lib{name}.so"), format!("{name}.so")]
    }
}

/// Open `path`, verify the export protocol and fingerprint, and keep the
/// create entry point.
#[allow(unsafe_code)]
fn load_library(name: &str, path: &Path) -> Result<LoadedPlugin> {
    let context = format!("dynamic plugin `{name}` ({})", path.display());
    // SAFETY: loading a library runs its initializers; the file was found
    // in a directory the embedder explicitly designated as trusted plugin
    // search path.
    let library = unsafe { Library::new(path) }
        .map_err(|error| plugin_error(&context, format!("cannot be loaded: {error}")))?;

    // SAFETY: the symbol names are NUL-terminated constants, and the
    // returned symbols are only dereferenced while `library` is alive
    // (which is forever — it is leaked below).
    let abi: Symbol<unsafe extern "C" fn() -> u32> =
        unsafe { library.get(ABI_SYMBOL) }.map_err(|error| {
            plugin_error(
                &context,
                format!("does not export `cordis_plugin_abi` (not a cordis plugin?): {error}"),
            )
        })?;
    // SAFETY: same lifetime argument as above.
    let fingerprint_sym: Symbol<unsafe extern "C" fn() -> *const c_char> =
        unsafe { library.get(FINGERPRINT_SYMBOL) }.map_err(|error| {
            plugin_error(
                &context,
                format!("does not export `cordis_plugin_fingerprint`: {error}"),
            )
        })?;
    // SAFETY: same lifetime argument as above.
    let create: Symbol<CreateFn> = unsafe { library.get(CREATE_SYMBOL) }.map_err(|error| {
        plugin_error(
            &context,
            format!("does not export `cordis_plugin_create`: {error}"),
        )
    })?;

    // SAFETY: these calls run foreign code, which the fingerprint check
    // below then constrains to code compiled exactly like this process.
    // A panic inside them aborts the process (extern "C" unwinding is
    // aborted by modern rustc); the macro-side catch_unwind prevents it
    // from ever getting that far.
    let reported_abi = unsafe { abi() };
    if reported_abi != DYNAMIC_ABI {
        return Err(plugin_error(
            &context,
            format!("reports ABI version {reported_abi}, this loader speaks {DYNAMIC_ABI}"),
        ));
    }
    // SAFETY: see above.
    let reported_fingerprint = unsafe { fingerprint_sym() };
    let reported_fingerprint = if reported_fingerprint.is_null() {
        String::new()
    } else {
        // SAFETY: the contract of `cordis_plugin_fingerprint` is a
        // NUL-terminated string owned by the library; reading it here is
        // within the library's lifetime.
        unsafe { CStr::from_ptr(reported_fingerprint) }
            .to_string_lossy()
            .into_owned()
    };
    if reported_fingerprint != fingerprint() {
        return Err(plugin_error(
            &context,
            format!(
                "was built for a different toolchain or cordis version; \
                 library fingerprint `{reported_fingerprint}` does not match \
                 this process' `{}`",
                fingerprint()
            ),
        ));
    }

    let create = *create;
    // SAFETY (deliberate leak): dropping the handle would `dlclose` the
    // library while plugin instances, vtables, and thread-local state may
    // still reference it — undefined behavior. Leaking keeps the mapping
    // alive for the whole process; unloading happens only at process exit
    // (worker restart), after full teardown. This also means a replaced
    // file is never re-loaded in the same process, by design.
    std::mem::forget(library);
    Ok(LoadedPlugin {
        create,
        path: path.to_path_buf(),
    })
}

/// Ask a loaded library for one plugin instance and adapt it.
#[allow(unsafe_code)]
fn create_instance(name: &str, loaded: &LoadedPlugin) -> Result<DynamicPlugin> {
    let context = format!("dynamic plugin `{name}` ({})", loaded.path.display());
    // SAFETY: `create` was recovered from a library whose fingerprint
    // matches this process byte for byte, so the returned box was allocated
    // by the same allocator with the same layout contract.
    let pointer = unsafe { (loaded.create)() };
    if pointer.is_null() {
        return Err(plugin_error(
            &context,
            "create returned null (constructor panicked?)",
        ));
    }
    // SAFETY: `cordis_plugin_create`'s contract is a double-boxed plugin
    // (`Box::into_raw(Box::new(PanicGuard { .. }))`); the fingerprint
    // check guarantees both sides compiled the same trait and box layout,
    // so reconstructing the box here is sound. We own the allocation.
    let boxed: Box<Box<dyn Plugin>> = unsafe { Box::from_raw(pointer as *mut Box<dyn Plugin>) };

    // The plugin-side PanicGuard guarantees these cannot panic (and must
    // not be caught here: a library's panic is foreign to this process'
    // std and would abort if unwound into our catch_unwind).
    Ok(DynamicPlugin { inner: *boxed })
}

/// The loader-side adapter: gives the plugin-owned box a local [`Plugin`]
/// implementation. Panics are already contained on the plugin side by
/// [`PanicGuard`]; this wrapper only forwards.
struct DynamicPlugin {
    inner: Box<dyn Plugin>,
}

impl Plugin for DynamicPlugin {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn inject(&self) -> &Inject {
        self.inner.inject()
    }

    fn validate_config(&self, config: Config) -> Result<Config> {
        self.inner.validate_config(config)
    }

    fn apply(&self, ctx: Context, config: Config) -> BoxFuture<Result<PluginOutput>> {
        self.inner.apply(ctx, config)
    }
}

fn plugin_error(context: &str, message: impl Into<String>) -> cordis::CordisError {
    cordis::CordisError::with_message(ErrorCode::Plugin, format!("{context}: {}", message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_include::PluginResolver as _;

    #[test]
    fn fingerprint_pins_every_abi_ingredient() {
        let fingerprint = fingerprint();
        assert!(fingerprint.contains("cordis-dynamic/"), "{fingerprint}");
        assert!(
            fingerprint.contains(&format!("cordis-rs/{VERSION}")),
            "{fingerprint}"
        );
        assert!(fingerprint.contains("rustc/"), "{fingerprint}");
        assert!(fingerprint.contains("target/"), "{fingerprint}");
        assert!(fingerprint.contains("panic/"), "{fingerprint}");
    }

    #[test]
    fn names_that_could_escape_the_directories_are_rejected() {
        for name in ["", ".", "..", "../escape", "a/b", "a\\b", "c:drive"] {
            assert!(validate_name(name).is_err(), "{name:?}");
        }
        for name in ["greeter", "my-plugin", "plugin_v2"] {
            assert!(validate_name(name).is_ok(), "{name:?}");
        }
    }

    #[test]
    fn library_file_names_follow_the_platform_scheme() {
        let names = library_file_names("greeter");
        if cfg!(target_os = "macos") {
            assert_eq!(names, ["libgreeter.dylib", "greeter.dylib"]);
        } else if cfg!(target_os = "windows") {
            assert_eq!(names, ["greeter.dll", "libgreeter.dll"]);
        } else {
            assert_eq!(names, ["libgreeter.so", "greeter.so"]);
        }
    }

    #[test]
    fn missing_libraries_report_unknown_plugin() {
        let resolver = DynamicPluginResolver::new([std::env::temp_dir()]);
        let error = resolver
            .resolve("definitely_not_a_real_plugin")
            .unwrap_err();
        assert!(
            error.to_string().contains("no plugin registered"),
            "{error}"
        );
    }

    #[test]
    fn traversal_names_are_rejected_before_touching_the_filesystem() {
        let resolver = DynamicPluginResolver::new(["/"]);
        let error = resolver.resolve("../etc/passwd").unwrap_err();
        assert!(
            error.to_string().contains("invalid dynamic plugin name"),
            "{error}"
        );
    }
}
