//! File watcher behavior (`watch` feature).
//!
//! Gated to Unix to avoid cross-platform filesystem-event flakiness in CI;
//! the implementation itself is platform-independent. Each test gets its
//! own directory so concurrent tests cannot observe each other's writes.

#![cfg(feature = "watch")]
#![cfg(unix)]

use cordis_include::{Document, EntryOptions, LoaderFile};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn isolated_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cordis-include-watch-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("config.yml")
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
    let path = isolated_dir("external");
    let file = LoaderFile::open(&path).unwrap();
    file.write(&Document::with_entries(vec![EntryOptions::new("a")]))
        .unwrap();

    let fired = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&fired);
    let _watcher = file
        .watch(move || signal.store(true, Ordering::SeqCst))
        .unwrap();

    // Let the watch register before touching the file.
    std::thread::sleep(Duration::from_millis(500));
    std::fs::write(&path, "entries:\n  - name: b\n    id: k2\n").unwrap();
    assert!(
        wait_for(&fired, Duration::from_secs(20)),
        "callback should fire after an external change"
    );
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn suspended_files_stay_silent() {
    let path = isolated_dir("suspended");
    let file = LoaderFile::open(&path).unwrap();
    file.write(&Document::with_entries(vec![EntryOptions::new("a")]))
        .unwrap();

    let fired = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&fired);
    let _watcher = file
        .watch(move || signal.store(true, Ordering::SeqCst))
        .unwrap();

    // Baseline: an external change reaches the callback, proving the watch
    // is live before we assert silence under suspension.
    std::thread::sleep(Duration::from_millis(500));
    std::fs::write(&path, "entries:\n  - name: b\n    id: k3\n").unwrap();
    assert!(
        wait_for(&fired, Duration::from_secs(20)),
        "callback should fire for the baseline change"
    );

    let fired = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&fired);
    let _watcher = file
        .watch(move || signal.store(true, Ordering::SeqCst))
        .unwrap();
    let _guard = file.suspend();
    std::fs::write(&path, "entries:\n  - name: c\n    id: k4\n").unwrap();
    std::thread::sleep(Duration::from_millis(2000));
    assert!(
        !fired.load(Ordering::SeqCst),
        "callback must stay silent while the file is suspended"
    );
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
