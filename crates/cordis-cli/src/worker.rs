//! Worker runtime: boot the loader, watch the config, exit on signals.

use cordis::Context;
use cordis_loader::{Loader, LoaderConfig, PluginRegistry};
use notify::{Config as NotifyConfig, RecursiveMode, Watcher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

/// Exit code asking the daemon for a hot restart.
pub const EXIT_RESTART: i32 = 51;
/// Exit code telling the daemon to quit without restarting.
pub const EXIT_QUIT: i32 = 52;
/// Exit code reporting that the loader never came up (bad config,
/// unreadable file); the daemon exits non-zero instead of masking it.
pub const EXIT_BOOT: i32 = 53;
/// Environment marker set by the daemon on workers it supervises. Such
/// workers watch their stdin pipe: the daemon holds the write end, so EOF
/// means the daemon is going away (clean shutdown or sudden death) and the
/// worker tears itself down gracefully.
pub const SUPERVISED_ENV: &str = "CORDIS_SUPERVISED";

/// Quiet window a plugin library must stay unchanged before the worker
/// restarts, so incremental linker output does not restart mid-write.
const PLUGIN_CHANGE_DEBOUNCE: Duration = Duration::from_millis(250);

/// Handle exposed as the `worker` service so plugins can stop or restart
/// the process (upstream's `ctx.loader.exit` / full-reload protocol).
pub struct WorkerHandle {
    inner: Arc<WorkerInner>,
}

impl WorkerHandle {
    /// Hot restart: dispose everything and ask the daemon for a new worker.
    pub fn restart(&self) -> ! {
        self.inner.teardown();
        std::process::exit(EXIT_RESTART);
    }

    /// Quit: dispose everything and tell the daemon not to restart.
    pub fn shutdown(&self) -> ! {
        self.inner.teardown();
        std::process::exit(EXIT_QUIT);
    }
}

/// Everything the worker owns; shared with the signal handler.
struct WorkerInner {
    root: Context,
    loader: Option<Loader>,
}

impl WorkerInner {
    fn teardown(&self) {
        if let Some(loader) = &self.loader {
            let _ = loader.dispose();
        }
        let _ = self.root.fiber().and_then(|fiber| fiber.dispose());
    }
}

/// Run the worker process: load dotenv, boot the loader (with dynamic
/// plugin directories, if any), watch the entry file and the plugin
/// directories, and block until a signal (or a `worker` service call)
/// exits the process. Never returns.
pub fn run(config_path: &Path, plugin_dirs: &[PathBuf]) -> ! {
    let root = Context::new();
    let mut registry = PluginRegistry::new();
    if !plugin_dirs.is_empty() {
        registry = registry.with_dynamic_dirs(plugin_dirs.iter());
    }
    let loader = match Loader::open(
        &root,
        LoaderConfig::new(config_path).with_registry(registry),
    ) {
        Ok(loader) => loader,
        Err(error) => {
            eprintln!(
                "cordis: failed to start from {}: {error}",
                config_path.display()
            );
            std::process::exit(EXIT_BOOT);
        }
    };
    let inner = Arc::new(WorkerInner {
        root: root.clone(),
        loader: Some(loader.clone()),
    });

    let handle = Arc::new(WorkerHandle {
        inner: Arc::clone(&inner),
    });
    if let Err(error) = root.provide_arc("worker", handle.clone()) {
        eprintln!("cordis: could not expose the worker service: {error}");
    }

    let signal_inner = Arc::clone(&inner);
    if ctrlc::set_handler(move || {
        eprintln!("cordis: signal received, shutting down");
        signal_inner.teardown();
        std::process::exit(EXIT_QUIT);
    })
    .is_err()
    {
        eprintln!("cordis: could not install signal handlers");
    }

    // Under the daemon, watch the supervisor's stdin pipe. EOF (or an
    // errored pipe) means the daemon is gone or asked us to stop — the
    // same graceful teardown a signal triggers, and the only notification
    // a SIGKILLed daemon can still send. Not spawned for manually-run
    // workers, so a terminal's stdin stays untouched.
    if std::env::var_os(SUPERVISED_ENV).is_some() {
        let inner = Arc::clone(&inner);
        let watched = std::thread::Builder::new()
            .name("cordis-supervisor-watch".to_owned())
            .spawn(move || {
                let mut stdin = std::io::stdin();
                let mut byte = [0_u8];
                loop {
                    match stdin.read(&mut byte) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => continue,
                    }
                }
                eprintln!("cordis: supervisor went away, shutting down");
                inner.teardown();
                std::process::exit(EXIT_QUIT);
            });
        if watched.is_err() {
            eprintln!("cordis: could not watch the supervisor pipe");
        }
    }

    match loader.watch() {
        Ok(_watcher) => {}
        Err(error) => eprintln!(
            "cordis: config hot reload disabled ({error}); restart manually to apply changes"
        ),
    }

    if !plugin_dirs.is_empty() {
        // A changed library only takes effect through a fresh worker: the
        // old process never unloads a mapped library, so the watcher asks
        // the daemon for a restart instead of reloading in place.
        match watch_plugin_dirs(plugin_dirs, handle) {
            Ok(()) => eprintln!(
                "cordis: dynamic plugins from {} (library changes hot-restart the worker)",
                plugin_dirs
                    .iter()
                    .map(|dir| dir.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Err(error) => eprintln!(
                "cordis: plugin library watching disabled ({error}); restart manually to apply changes"
            ),
        }
    }

    if let Some(error) = loader.last_error() {
        eprintln!("cordis: startup issue: {error}");
    }

    eprintln!(
        "cordis: worker ready ({} entries, config: {})",
        loader.tree().entries().len(),
        config_path.display()
    );

    // The worker's work happens on fiber threads and watcher callbacks;
    // park until a signal or service call ends the process.
    loop {
        std::thread::park();
    }
}

/// Watch `dirs` (non-recursively) for plugin library changes and hot
/// restart the worker through `handle` once a change settles.
///
/// Only files with a dynamic-library extension count; matching by
/// extension (instead of exact paths) also absorbs macOS FSEvents
/// reporting realpaths. The thread ends the process on the first settled
/// change, so there is nothing to return.
fn watch_plugin_dirs(dirs: &[PathBuf], handle: Arc<WorkerHandle>) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::RecommendedWatcher::new(tx, NotifyConfig::default())
        .map_err(|error| format!("cannot create watcher: {error}"))?;
    for dir in dirs {
        watcher
            .watch(dir, RecursiveMode::NonRecursive)
            .map_err(|error| format!("cannot watch {}: {error}", dir.display()))?;
    }

    std::thread::Builder::new()
        .name("cordis-plugin-watch".to_owned())
        .spawn(move || {
            let _watcher = watcher; // keep the watch alive for the thread's lifetime
            let mut pending = false;
            loop {
                match rx.recv_timeout(PLUGIN_CHANGE_DEBOUNCE) {
                    Ok(Ok(event)) => {
                        if event.paths.iter().any(|path| is_plugin_library(path)) {
                            pending = true;
                        }
                    }
                    Ok(Err(_)) => {}
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if pending {
                            eprintln!("cordis: plugin library changed, restarting worker");
                            handle.restart();
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .map_err(|error| format!("cannot spawn watcher thread: {error}"))?;
    Ok(())
}

/// Whether `path` looks like a dynamic library by extension.
fn is_plugin_library(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        matches!(
            extension.to_ascii_lowercase().to_str(),
            Some("so" | "dylib" | "dll")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_library_extensions_match() {
        assert!(is_plugin_library(Path::new("libgreeter.so")));
        assert!(is_plugin_library(Path::new("libgreeter.dylib")));
        assert!(is_plugin_library(Path::new("greeter.dll")));
        assert!(is_plugin_library(Path::new("GREETER.SO")));
        assert!(!is_plugin_library(Path::new("cordis.yml")));
        assert!(!is_plugin_library(Path::new("libgreeter.so.tmp")));
        assert!(!is_plugin_library(Path::new("plugins")));
    }
}
