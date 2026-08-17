//! Bake the build fingerprint ingredients for the `dynamic` feature into
//! env vars readable via `env!` from `src`.
//!
//! The vars are emitted unconditionally (they are inert without the
//! feature) and are part of the `cordis_plugin_fingerprint()` protocol:
//! a dynamic library is only accepted when every ingredient matches the
//! loading process byte for byte.

use std::process::Command;

fn main() {
    // No `rerun-if` directives: rerun on any package change (the default),
    // and the build script itself is recompiled — hence re-run — whenever
    // the toolchain changes.
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_owned());
    let (release, hash) = rustc_version(&rustc);
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown-target".to_owned());
    let panic = std::env::var("CARGO_CFG_PANIC").unwrap_or_else(|_| "unknown-panic".to_owned());
    println!("cargo:rustc-env=CORDIS_RUSTC_RELEASE={release}");
    println!("cargo:rustc-env=CORDIS_RUSTC_COMMIT_HASH={hash}");
    println!("cargo:rustc-env=CORDIS_BUILD_TARGET={target}");
    println!("cargo:rustc-env=CORDIS_BUILD_PANIC={panic}");
}

/// The `release:` and `commit-hash:` fields of `rustc -vV`, falling back to
/// `unknown` markers when the compiler cannot be queried.
fn rustc_version(rustc: &str) -> (String, String) {
    let output = Command::new(rustc)
        .arg("-vV")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default();
    let field = |name: &str| {
        output
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != "unknown")
            .map(ToString::to_string)
    };
    (
        field("release: ").unwrap_or_else(|| "unknown-release".to_owned()),
        field("commit-hash: ").unwrap_or_else(|| "unknown-hash".to_owned()),
    )
}
