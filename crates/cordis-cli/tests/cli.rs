//! End-to-end run of the `cordis` binary (Unix: uses the kill utility).

#![cfg(unix)]

use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

fn temp_dir(stem: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cordis-cli-test-{stem}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn kill(child: &std::process::Child, signal: &str) {
    let _ = Command::new("kill")
        .args([signal, &child.id().to_string()])
        .status();
}

#[test]
fn run_boots_the_loader_and_sigterm_exits_gracefully() {
    let dir = temp_dir("sigterm");
    let config = dir.join("cordis.yml");
    std::fs::write(&config, "entries:\n  - id: g\n    name: group\n").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_cordis"))
        .arg("run")
        .arg(&config)
        .process_group(0)
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn cordis binary");

    // Give the daemon and its worker a moment to boot.
    std::thread::sleep(Duration::from_millis(800));
    if let Ok(Some(status)) = child.try_wait() {
        panic!("cordis exited early with {status}");
    }

    // Signal the whole process group, like a terminal Ctrl+C would. The
    // `--` separator keeps the negative pgid from being parsed as a flag.
    let _ = Command::new("kill")
        .args(["-TERM", "--", &format!("-{}", child.id())])
        .status();
    let output = child.wait_with_output().expect("wait for cordis");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("worker ready"), "stderr: {stderr}");
    assert!(stderr.contains("shutting down"), "stderr: {stderr}");
    assert_eq!(output.status.code(), Some(0), "daemon should exit 0");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_config_boots_an_empty_loader() {
    let dir = temp_dir("missing");
    let config = dir.join("does-not-exist.yml");

    let mut child = Command::new(env!("CARGO_BIN_EXE_cordis"))
        .args(["run", "--worker"])
        .arg(&config)
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn cordis worker");

    std::thread::sleep(Duration::from_millis(800));
    let early = child
        .try_wait()
        .expect("poll child")
        .map(|status| panic!("worker exited early with {status}"));
    assert!(early.is_none(), "worker stays parked on an empty config");

    kill(&child, "-KILL");
    let output = child.wait_with_output().expect("reap worker");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("worker ready (0 entries"),
        "stderr: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unknown_arguments_fail_fast() {
    let output = Command::new(env!("CARGO_BIN_EXE_cordis"))
        .arg("frobnicate")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown command"), "stderr: {stderr}");
}
