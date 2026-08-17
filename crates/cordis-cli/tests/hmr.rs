//! End-to-end HMR: replacing a plugin library in a watched `--plugin-dir`
//! hot-restarts the worker (exit 51) and the new build takes effect.
//!
//! Unix only: it replaces a library file that a running process still has
//! mapped, which Windows forbids. Uses the kill utility on process groups
//! like the other CLI tests.

#![cfg(unix)]

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

fn temp_dir(stem: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cordis-hmr-test-{stem}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The workspace `target/` directory holding the built rlibs.
fn target_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<repo>/crates/cordis-cli`.
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .ancestors()
                .nth(2)
                .unwrap()
                .join("target")
        })
}

/// The rustc that built this test binary: the pinned `RUSTC` override if
/// present, else the binary next to `cargo`, else PATH.
fn rustc() -> PathBuf {
    if let Some(rustc) = std::env::var_os("RUSTC") {
        let rustc = PathBuf::from(rustc);
        if rustc.is_file() {
            return rustc;
        }
    }
    let sibling = PathBuf::from(env!("CARGO")).parent().unwrap().join("rustc");
    if sibling.is_file() {
        return sibling;
    }
    PathBuf::from("rustc")
}

/// Locate the built rlib for a workspace crate, preferring the unhashed
/// uplift in `target/debug/` and falling back to the newest hashed copy.
fn find_rlib(name: &str) -> PathBuf {
    let debug = target_dir().join("debug");
    let uplifted = debug.join(format!("lib{name}.rlib"));
    if uplifted.is_file() {
        return uplifted;
    }
    let prefix = format!("lib{name}-");
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(debug.join("deps")).expect("deps directory") {
        let entry = entry.unwrap();
        let path = entry.path();
        let is_match = path
            .extension()
            .is_some_and(|extension| extension == "rlib")
            && path
                .file_name()
                .and_then(|file| file.to_str())
                .is_some_and(|file| file.starts_with(&prefix));
        if !is_match {
            continue;
        }
        let mtime = entry.metadata().unwrap().modified().unwrap();
        if newest.as_ref().is_none_or(|(best, _)| mtime > *best) {
            newest = Some((mtime, path));
        }
    }
    newest
        .unwrap_or_else(|| panic!("no rlib found for {name}"))
        .1
}

/// The file a `cdylib` crate named `crate_name` produces.
fn cdylib_file_name(crate_name: &str) -> String {
    if cfg!(target_os = "windows") {
        format!("{crate_name}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{crate_name}.dylib")
    } else {
        format!("lib{crate_name}.so")
    }
}

/// Compile `tests/fixtures/file_writer.rs` into a `cdylib` in `out_dir`,
/// with extra `--cfg` flags to select the build tag.
fn compile_plugin(out_dir: &Path, crate_name: &str, cfgs: &[&str]) -> PathBuf {
    std::fs::create_dir_all(out_dir).unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/file_writer.rs");
    let mut command = Command::new(rustc());
    command
        .args(["--edition", "2024", "--crate-type", "cdylib"])
        .arg(format!("--crate-name={crate_name}"))
        .arg("--out-dir")
        .arg(out_dir)
        .arg(&source)
        .arg("-L")
        .arg(format!(
            "dependency={}",
            target_dir().join("debug/deps").display()
        ))
        .arg("--extern")
        .arg(format!(
            "cordis_loader={}",
            find_rlib("cordis_loader").display()
        ));
    for cfg in cfgs {
        command.arg(format!("--cfg={cfg}"));
    }
    let output = command.output().expect("run rustc");
    assert!(
        output.status.success(),
        "compiling plugin {crate_name} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    out_dir.join(cdylib_file_name(crate_name))
}

fn wait_for(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn replacing_a_plugin_library_hot_restarts_the_worker() {
    let dir = temp_dir("replace");
    let plugins = dir.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let build = dir.join("build");
    let v1 = compile_plugin(&build, "file_writer", &[]);
    let v2 = compile_plugin(&build, "file_writer_v2", &["hmr_v2"]);
    std::fs::copy(&v1, plugins.join(cdylib_file_name("file_writer"))).unwrap();

    let out = dir.join("out.txt");
    let config = dir.join("cordis.json");
    std::fs::write(
        &config,
        format!(
            r#"{{"entries":[{{"id":"w","name":"file_writer","config":{{"out":"{}"}}}}]}}"#,
            out.display()
        ),
    )
    .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_cordis"))
        .arg("run")
        .arg(&config)
        .arg("--plugin-dir")
        .arg(&plugins)
        .process_group(0)
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn cordis binary");

    wait_for("the first build to write v1", || {
        std::fs::read_to_string(&out).ok().as_deref() == Some("v1")
    });

    // Replace the library atomically, from outside the watched directory
    // so the staged copy does not trigger an early restart.
    let staged = dir.join("staged.so");
    std::fs::copy(&v2, &staged).unwrap();
    std::fs::rename(&staged, plugins.join(cdylib_file_name("file_writer"))).unwrap();

    // The watcher restarts the worker (exit 51) and the daemon respawns
    // it; the fresh process loads the new build and writes v2.
    wait_for("the restarted worker to write v2", || {
        std::fs::read_to_string(&out).ok().as_deref() == Some("v2")
    });
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
    assert!(
        stderr.contains("plugin library changed, restarting worker"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("respawning"), "stderr: {stderr}");
    assert_eq!(output.status.code(), Some(0), "daemon should exit 0");

    let _ = std::fs::remove_dir_all(&dir);
}
