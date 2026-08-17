//! File watcher behavior (`watch` feature).
//!
//! Gated to Unix to avoid cross-platform filesystem-event flakiness in CI;
//! the implementation itself is platform-independent.

#![cfg(feature = "watch")]
#![cfg(unix)]

use cordis_include::{Document, EntryOptions, LoaderFile};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn temp_path(stem: &str, ext: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "cordis-include-watch-{stem}-{}.{ext}",
        std::process::id()
    ))
}

fn wait_for(flag: &Arc<AtomicBool>, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if flag.load(Ordering::SeqCst) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    flag.load(Ordering::SeqCst)
}

#[test]
fn external_change_fires_the_callback() {
    let path = temp_path("external", "yml");
    let _ = std::fs::remove_file(&path);
    let file = LoaderFile::open(&path).unwrap();
    file.write(&Document::with_entries(vec![EntryOptions::new("a")]))
        .unwrap();

    let fired = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&fired);
    let _watcher = file
        .watch(move || signal.store(true, Ordering::SeqCst))
        .unwrap();

    // Let the watch register before touching the file.
    std::thread::sleep(Duration::from_millis(300));
    std::fs::write(&path, "entries:\n  - name: b\n    id: k2\n").unwrap();
    assert!(
        wait_for(&fired, Duration::from_secs(10)),
        "callback should fire after an external change"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn suspended_files_stay_silent() {
    let path = temp_path("suspended", "yml");
    let _ = std::fs::remove_file(&path);
    let file = LoaderFile::open(&path).unwrap();
    file.write(&Document::with_entries(vec![EntryOptions::new("a")]))
        .unwrap();

    let fired = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&fired);
    let _watcher = file
        .watch(move || signal.store(true, Ordering::SeqCst))
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));

    let _guard = file.suspend();
    std::fs::write(&path, "entries:\n  - name: b\n    id: k3\n").unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        !fired.load(Ordering::SeqCst),
        "callback must stay silent while the file is suspended"
    );
    let _ = std::fs::remove_file(&path);
}
