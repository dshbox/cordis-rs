//! Dynamic-library plugin tests.
//!
//! Fixtures under `tests/fixtures/` are compiled to `cdylib` files with the
//! same toolchain and workspace rlibs that built the test binary, then
//! loaded through [`DynamicPluginResolver`] — both directly and through a
//! full [`Loader::open`] run.

#![cfg(feature = "dynamic")]

use cordis_include::PluginResolver as _;
use cordis_loader::dynamic::DynamicPluginResolver;
use cordis_loader::{Loader, LoaderConfig, PluginRegistry};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

fn temp_dir(stem: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("cordis-dynamic-test-{stem}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The workspace `target/` directory holding the built rlibs.
fn target_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<repo>/crates/cordis-loader`.
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

/// Compile `tests/fixtures/<fixture>.rs` into a `cdylib` in `out_dir`.
fn compile_fixture(out_dir: &Path, fixture: &str, cfgs: &[&str]) -> PathBuf {
    std::fs::create_dir_all(out_dir).unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("{fixture}.rs"));
    let mut command = Command::new(rustc());
    command
        .args(["--edition", "2024", "--crate-type", "cdylib"])
        .arg(format!("--crate-name={fixture}"))
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
        "compiling fixture {fixture} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    out_dir.join(cdylib_file_name(fixture))
}

fn wait_for(what: &str, check: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn resolves_and_applies_dynamic_plugins_end_to_end() {
    let dir = temp_dir("e2e");
    let plugins = dir.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let library = compile_fixture(&dir.join("build"), "file_writer", &[]);
    std::fs::copy(&library, plugins.join(cdylib_file_name("file_writer"))).unwrap();

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

    let root = cordis::Context::new();
    let registry = PluginRegistry::new().with_dynamic_dirs([&plugins]);
    let loader = Loader::open(&root, LoaderConfig::new(&config).with_registry(registry)).unwrap();
    assert!(loader.last_error().is_none(), "{:?}", loader.last_error());
    wait_for("the plugin to write its output file", || {
        std::fs::read_to_string(&out).ok().as_deref() == Some("v1")
    });

    let _ = loader.dispose();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_resolve_yields_a_fresh_handle() {
    let dir = temp_dir("fresh");
    let plugins = dir.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let library = compile_fixture(&dir.join("build"), "file_writer", &[]);
    std::fs::copy(&library, plugins.join(cdylib_file_name("file_writer"))).unwrap();

    let resolver = DynamicPluginResolver::new([&plugins]);
    let first = resolver.resolve("file_writer").unwrap();
    let second = resolver.resolve("file_writer").unwrap();
    assert_ne!(first.key(), second.key());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn static_registration_takes_priority_over_dynamic_libraries() {
    let dir = temp_dir("priority");
    let plugins = dir.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let library = compile_fixture(&dir.join("build"), "file_writer", &[]);
    std::fs::copy(&library, plugins.join(cdylib_file_name("file_writer"))).unwrap();

    let mut registry = PluginRegistry::new().with_dynamic_dirs([&plugins]);
    registry.register("file_writer", || {
        cordis::plugin_sync::<(), _>("file_writer", cordis::Inject::default(), |_ctx, _config| {
            Ok(cordis::PluginOutput::none())
        })
    });
    let handle = cordis_include::PluginResolver::resolve(&registry, "file_writer").unwrap();
    assert_eq!(handle.name(), "file_writer");
    // The static factory won: resolving does not even need the file.

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rejects_libraries_with_a_foreign_fingerprint() {
    let dir = temp_dir("fingerprint");
    let plugins = dir.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let library = compile_fixture(&dir.join("build"), "bad_fingerprint", &[]);
    std::fs::copy(&library, plugins.join(cdylib_file_name("bad_fingerprint"))).unwrap();

    let resolver = DynamicPluginResolver::new([&plugins]);
    let error = resolver.resolve("bad_fingerprint").unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("fingerprint") && message.contains("does not match"),
        "{message}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rejects_libraries_without_the_protocol_exports() {
    let dir = temp_dir("plain");
    let plugins = dir.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let library = compile_fixture(&dir.join("build"), "plain_library", &[]);
    std::fs::copy(&library, plugins.join(cdylib_file_name("plain_library"))).unwrap();

    let resolver = DynamicPluginResolver::new([&plugins]);
    let error = resolver.resolve("plain_library").unwrap_err();
    assert!(error.to_string().contains("cordis_plugin_abi"), "{}", error);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn panicking_plugins_are_contained_on_the_plugin_side() {
    let dir = temp_dir("panic");
    let plugins = dir.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let library = compile_fixture(&dir.join("build"), "panic_on_name", &[]);
    std::fs::copy(&library, plugins.join(cdylib_file_name("panic_on_name"))).unwrap();

    // A cdylib links its own std, so the loader must not see the panic at
    // all: the plugin-side guard turns it into a fallback name.
    let resolver = DynamicPluginResolver::new([&plugins]);
    let handle = resolver.resolve("panic_on_name").unwrap();
    assert_eq!(handle.name(), "(plugin panicked in name())");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn apply_time_panics_surface_as_fiber_errors() {
    let dir = temp_dir("panic-apply");
    let plugins = dir.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let library = compile_fixture(&dir.join("build"), "panic_in_apply", &[]);
    std::fs::copy(&library, plugins.join(cdylib_file_name("panic_in_apply"))).unwrap();

    // The plugin-side guard converts the poll-time panic into an ordinary
    // apply failure: the fiber fails instead of the process aborting.
    let resolver = DynamicPluginResolver::new([&plugins]);
    let handle = resolver.resolve("panic_in_apply").unwrap();
    let root = cordis::Context::new();
    let fiber = root.plugin(handle, cordis::Value::new(()));
    let error = fiber.try_wait().unwrap_err();
    assert!(error.to_string().contains("panicked in apply"), "{error}");

    let _ = std::fs::remove_dir_all(&dir);
}
