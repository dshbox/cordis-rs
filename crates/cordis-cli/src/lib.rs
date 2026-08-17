//! Command-line runner for the [cordis-rs](https://crates.io/crates/cordis-rs)
//! plugin framework: `cordis run <config.yml>`.
//!
//! The process model follows upstream Cordis' NodeLoader: `cordis run`
//! supervises a worker subprocess running the loader. The worker exits
//! with code `51` to request a hot restart and `52` to quit; `SIGINT` /
//! `SIGTERM` dispose the root context gracefully and exit `52`. `.env` and
//! `.env.local` are loaded (without overriding existing variables) before
//! the worker boots, and the entry file is watched for hot reload.
//!
//! `--plugin-dir <dir>` (repeatable) additionally resolves entries from
//! dynamic-library plugins compiled against the same toolchain; a change
//! to a library in those directories hot-restarts the worker so the new
//! build is loaded by a fresh process.
//!
//! Plugins reach the process controls through the `worker` service
//! ([`worker::WorkerHandle`]): `restart()` maps to a full worker reload,
//! and `shutdown()` stops the whole application.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod dotenv;
pub mod worker;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// What the supervisor does after a worker exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Spawn a fresh worker (hot restart).
    Restart,
    /// Stop supervising.
    Stop,
}

/// Decide whether to restart the worker after it exited with `exit_code`.
///
/// Only exit code [`worker::EXIT_RESTART`] (51) restarts, and never once
/// the daemon itself received a shutdown signal.
pub fn supervisor_action(exit_code: Option<i32>, shutdown: bool) -> Action {
    if shutdown || exit_code != Some(worker::EXIT_RESTART) {
        Action::Stop
    } else {
        Action::Restart
    }
}

/// Parsed command line.
#[derive(Debug, PartialEq, Eq)]
pub struct Options {
    /// Entry config file.
    pub config: PathBuf,
    /// Directories searched for dynamic-library plugins (repeatable).
    pub plugin_dirs: Vec<PathBuf>,
    /// Run as the daemon's worker process instead of supervising.
    pub worker: bool,
}

/// Parse `cordis` arguments (after the binary name).
pub fn parse_args(args: &[String]) -> Result<Options, String> {
    let Some(command) = args.first() else {
        return Err(usage());
    };
    if command != "run" {
        return Err(format!("unknown command `{command}`\n\n{}", usage()));
    }
    let mut config = None;
    let mut plugin_dirs = Vec::new();
    let mut worker = false;
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--worker" | "-w" => worker = true,
            "--help" | "-h" => return Err(usage()),
            "--plugin-dir" => {
                let Some(value) = rest.next() else {
                    return Err(format!("--plugin-dir requires a directory\n\n{}", usage()));
                };
                plugin_dirs.push(PathBuf::from(value));
            }
            other if other.starts_with("--plugin-dir=") => {
                let value = other.strip_prefix("--plugin-dir=").unwrap();
                if value.is_empty() {
                    return Err(format!("--plugin-dir requires a directory\n\n{}", usage()));
                }
                plugin_dirs.push(PathBuf::from(value));
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown flag `{other}`\n\n{}", usage()));
            }
            other => {
                if config.replace(PathBuf::from(other)).is_some() {
                    return Err(format!(
                        "unexpected extra argument `{other}`\n\n{}",
                        usage()
                    ));
                }
            }
        }
    }
    match config {
        Some(config) => Ok(Options {
            config,
            plugin_dirs,
            worker,
        }),
        None => Err(format!("missing config file\n\n{}", usage())),
    }
}

fn usage() -> String {
    "usage: cordis run <config.yml> [--plugin-dir <dir>]... [--worker]

  run             start the loader from an entry config file
  --plugin-dir    also resolve plugins from dynamic libraries in <dir>
                  (repeatable); library changes there hot-restart the worker
  --worker        internal: run as the daemon's worker process

Worker exit codes: 51 = hot restart, 52 = quit."
        .to_owned()
}

/// Entry point used by the `cordis` binary: parse arguments, load dotenv,
/// then supervise or run as the worker.
pub fn run<I, S>(args: I) -> i32
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args: Vec<String> = args.into_iter().map(Into::into).collect();
    let options = match parse_args(&args) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return 2;
        }
    };
    if let Ok(dir) = std::env::current_dir() {
        dotenv::load(&dir);
    }
    if options.worker {
        worker::run(&options.config, &options.plugin_dirs);
    }
    supervise(&options.config, &options.plugin_dirs)
}

/// Supervise worker subprocesses until they quit or a signal arrives.
fn supervise(config: &std::path::Path, plugin_dirs: &[PathBuf]) -> i32 {
    let shutdown = Arc::new(AtomicBool::new(false));
    let signal_flag = Arc::clone(&shutdown);
    if ctrlc::set_handler(move || {
        eprintln!("cordis: shutdown requested");
        signal_flag.store(true, Ordering::SeqCst);
    })
    .is_err()
    {
        eprintln!("cordis: could not install signal handlers");
    }

    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("cordis: cannot resolve own executable: {error}");
            return 1;
        }
    };
    loop {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
        let mut command = std::process::Command::new(&exe);
        command.arg("run").arg(config).arg("--worker");
        for dir in plugin_dirs {
            command.arg("--plugin-dir").arg(dir);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                eprintln!("cordis: cannot spawn worker: {error}");
                return 1;
            }
        };
        let exit_code = child.wait().ok().and_then(|status| status.code());
        if supervisor_action(exit_code, shutdown.load(Ordering::SeqCst)) == Action::Stop {
            break;
        }
        eprintln!("cordis: worker requested restart, respawning");
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn parses_run_command_and_flags() {
        assert_eq!(
            parse_args(&args(&["run", "cordis.yml"])).unwrap(),
            Options {
                config: "cordis.yml".into(),
                plugin_dirs: Vec::new(),
                worker: false,
            }
        );
        assert_eq!(
            parse_args(&args(&["run", "cordis.yml", "--worker"])).unwrap(),
            Options {
                config: "cordis.yml".into(),
                plugin_dirs: Vec::new(),
                worker: true,
            }
        );
    }

    #[test]
    fn parses_plugin_dirs_in_both_forms() {
        let options = parse_args(&args(&[
            "run",
            "cordis.yml",
            "--plugin-dir",
            "a",
            "--plugin-dir=b",
        ]))
        .unwrap();
        assert_eq!(
            options.plugin_dirs,
            [PathBuf::from("a"), PathBuf::from("b")]
        );
        assert!(!options.worker);
    }

    #[test]
    fn rejects_malformed_plugin_dirs() {
        assert!(parse_args(&args(&["run", "cordis.yml", "--plugin-dir"])).is_err());
        assert!(parse_args(&args(&["run", "cordis.yml", "--plugin-dir="])).is_err());
    }

    #[test]
    fn rejects_missing_or_unknown_arguments() {
        assert!(parse_args(&args(&[])).is_err());
        assert!(parse_args(&args(&["start", "cordis.yml"])).is_err());
        assert!(parse_args(&args(&["run"])).is_err());
        assert!(parse_args(&args(&["run", "a.yml", "b.yml"])).is_err());
        assert!(parse_args(&args(&["run", "a.yml", "--nope"])).is_err());
    }

    #[test]
    fn only_code_51_restarts_and_never_after_shutdown() {
        assert_eq!(supervisor_action(Some(51), false), Action::Restart);
        assert_eq!(supervisor_action(Some(51), true), Action::Stop);
        assert_eq!(supervisor_action(Some(52), false), Action::Stop);
        assert_eq!(supervisor_action(Some(0), false), Action::Stop);
        assert_eq!(supervisor_action(None, false), Action::Stop);
    }
}
