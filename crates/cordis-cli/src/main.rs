//! The `cordis` binary: see [`cordis_cli`] for the actual implementation.

use std::ffi::OsString;

fn main() {
    // OsStrings all the way down: a config path with non-UTF-8 bytes (legal
    // on Unix filesystems) must reach the loader unchanged instead of being
    // mangled into replacement characters.
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    std::process::exit(cordis_cli::run(args));
}
