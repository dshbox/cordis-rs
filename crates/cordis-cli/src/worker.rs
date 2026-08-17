//! Worker runtime: boot the loader, watch the config, exit on signals.

use cordis::Context;
use cordis_loader::{Loader, LoaderConfig};
use std::path::Path;
use std::sync::Arc;

/// Exit code asking the daemon for a hot restart.
pub const EXIT_RESTART: i32 = 51;
/// Exit code telling the daemon to quit without restarting.
pub const EXIT_QUIT: i32 = 52;

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

/// Run the worker process: load dotenv, boot the loader, watch the entry
/// file, and block until a signal (or a `worker` service call) exits the
/// process. Never returns.
pub fn run(config_path: &Path) -> ! {
    let root = Context::new();
    let loader = match Loader::open(&root, LoaderConfig::new(config_path)) {
        Ok(loader) => loader,
        Err(error) => {
            eprintln!(
                "cordis: failed to start from {}: {error}",
                config_path.display()
            );
            std::process::exit(EXIT_QUIT);
        }
    };
    let inner = Arc::new(WorkerInner {
        root: root.clone(),
        loader: Some(loader.clone()),
    });

    let handle = Arc::new(WorkerHandle {
        inner: Arc::clone(&inner),
    });
    if let Err(error) = root.provide_arc("worker", handle) {
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

    match loader.watch() {
        Ok(_watcher) => {}
        Err(error) => eprintln!(
            "cordis: config hot reload disabled ({error}); restart manually to apply changes"
        ),
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
