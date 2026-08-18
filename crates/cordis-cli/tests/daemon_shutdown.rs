//! Daemon shutdown forwarding: a signal delivered to the daemon alone must
//! take the worker down with it (issue #31), instead of hanging in
//! `child.wait()` while the worker keeps running orphaned.

// The orphan check walks /proc, so the whole test is Linux-only.
#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Marker inside every process command line that belongs to this test run.
fn unique_stem() -> String {
    format!(
        "cordis-cli-shutdown-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as u64)
            .unwrap_or(0)
    )
}

/// Spawn the daemon on an `entries: []` config; panics unless the worker
/// reports readiness on stderr within `timeout`.
// The Child is the caller's to reap; only the panic path kills it here.
#[allow(clippy::zombie_processes)]
fn spawn_ready_daemon(stem: &str) -> (Child, PathBuf) {
    let dir = std::env::temp_dir().join(stem);
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("cordis.yml");
    std::fs::write(&config, "entries: []\n").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_cordis"))
        .arg("run")
        .arg(&config)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cordis daemon");

    let stderr = child.stderr.take().expect("piped stderr");
    let seen = Arc::new(Mutex::new(String::new()));
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            log.lock().unwrap().push_str(&line);
            log.lock().unwrap().push('\n');
        }
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if seen.lock().unwrap().contains("worker ready") {
            return (child, config);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    panic!(
        "worker never reported ready; daemon stderr:\n{}",
        seen.lock().unwrap()
    );
}

/// Processes whose command line still references this test's config path —
/// the daemon, the worker, or strays from an earlier run.
fn processes_referencing(stem: &str) -> usize {
    let mut count = 0;
    let entries = std::fs::read_dir("/proc").unwrap();
    for entry in entries.flatten() {
        let Ok(cmdline) = std::fs::read_to_string(entry.path().join("cmdline")) else {
            continue;
        };
        // /proc cmdline entries are NUL-separated; compare loosely.
        if cmdline.contains(stem) {
            count += 1;
        }
    }
    count
}

/// `kill -TERM <daemon>` only: the daemon must forward the shutdown to the
/// worker (gracefully, via the supervisor pipe), exit 0 itself within a
/// bounded time, and leave no orphan worker behind.
#[cfg(target_os = "linux")]
#[test]
fn sigterm_to_daemon_only_takes_the_worker_down() {
    let stem = unique_stem();
    let (mut daemon, _config) = spawn_ready_daemon(&stem);

    let pid = daemon.id() as i32;
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }

    // The daemon exits promptly (it used to hang in child.wait() forever).
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match daemon.try_wait().expect("daemon is waitable") {
            Some(status) => {
                assert_eq!(status.code(), Some(0), "daemon exit code after SIGTERM");
                break;
            }
            None if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            None => panic!("daemon did not exit within 5s of SIGTERM"),
        }
    }

    // And no worker survives it.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if processes_referencing(&stem) == 0 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "worker processes survived the daemon shutdown"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join(&stem));
}
