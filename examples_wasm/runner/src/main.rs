//! Host runner for guest Components stored in a directory.
//!
//! The runner owns Cordis's Context and Fiber lifecycle. Guests are ordinary
//! `cordis:plugin` Components: a `.wasm` file is one artifact, and an optional
//! sibling `.config` file is its immutable generation input.
//!
//! This is a local inspection runner, not a production admission boundary: it
//! accepts arbitrary local artifacts and has no signature/provenance policy or
//! memory limiter. The Component host surface is narrow, but that is not a
//! substitute for a deployment trust policy.

use std::ffi::OsStr;
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

use cordis_core::logger::{Exporter, LogRecord};
use cordis_core::{BoxError, Context, Level, Plugin, PreparedPlugin, Routing};
use cordis_wasm::HostEvent;
use cordis_wasm::{ComponentArtifact, ComponentInput, ComponentPlugin};

const DEFAULT_MODULE_DIRECTORY: &str = "examples_wasm/modules";

struct Module {
    path: PathBuf,
    input: ComponentInput,
}

/// The runner's own diagnostics sink; Components cannot install exporters.
struct ConsoleExporter;

impl Exporter for ConsoleExporter {
    fn export(&self, record: &LogRecord) {
        println!("  [{}] {}", record.level().as_str(), record.text());
    }

    fn default_level(&self) -> Level {
        Level::Debug
    }
}

fn module_directory() -> PathBuf {
    std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_MODULE_DIRECTORY))
}

fn load_modules(plugin: &ComponentPlugin, directory: &Path) -> Result<Vec<Module>, BoxError> {
    // Directory membership is the only admission policy in this preview tool.
    // Production loading needs explicit provenance and resource limits.
    let mut paths = fs::read_dir(directory)?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| path.extension() == Some(OsStr::new("wasm")))
        .collect::<Vec<_>>();
    paths.sort();

    if paths.is_empty() {
        return Err(format!("no .wasm Components found in {}", directory.display()).into());
    }

    paths
        .into_iter()
        .map(|path| {
            let configuration_path = path.with_extension("config");
            let artifact = ComponentArtifact::from_bytes(fs::read(&path)?);
            let artifact = if configuration_path.is_file() {
                artifact.with_configuration(fs::read(configuration_path)?)
            } else {
                artifact
            };
            Ok(Module {
                input: plugin.prepare(artifact)?,
                path,
            })
        })
        .collect()
}

async fn dispose_all(fibers: &[cordis_core::FiberHandle]) {
    for fiber in fibers.iter().rev() {
        if let Err(error) = fiber.dispose().await {
            eprintln!("failed to dispose {}: {error}", fiber.name());
        }
    }
}

/// Split one console line into the Event name and its UTF-8 payload.
fn parse_event(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    (!line.is_empty()).then(|| line.split_once(' ').unwrap_or((line, "")))
}

/// Read console lines without enabling Tokio's I/O feature in the workspace.
fn console_lines() -> tokio::sync::mpsc::UnboundedReceiver<std::io::Result<String>> {
    let (send, receive) = tokio::sync::mpsc::unbounded_channel();
    thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            if send.send(line).is_err() {
                break;
            }
        }
    });
    receive
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let directory = module_directory();
    let plugin = ComponentPlugin::new()?;
    let modules = load_modules(&plugin, &directory)?;
    let context = Context::new();
    let _console = context.add_exporter(Arc::new(ConsoleExporter))?;
    let mut fibers = Vec::with_capacity(modules.len());

    println!(
        "loading {} Component(s) from {}",
        modules.len(),
        directory.display()
    );
    for module in modules {
        match context
            .spawn(PreparedPlugin::from_input(plugin.clone(), module.input))
            .await
        {
            Ok(fiber) => {
                println!("  ✓ {}", module.path.display());
                fibers.push(fiber);
            }
            Err(error) => {
                dispose_all(&fibers).await;
                return Err(format!("could not start {}: {error}", module.path.display()).into());
            }
        }
    }

    println!("all Components active; type '<event> <payload>' or 'quit' to dispose them");
    let mut lines = console_lines();
    while let Some(line) = lines.recv().await {
        let line = line?;
        if line.trim() == "quit" {
            break;
        }
        if let Some((name, payload)) = parse_event(&line) {
            context
                .emit::<HostEvent>(Routing::Unscoped, HostEvent::new(name, payload.as_bytes()))
                .await?;
        }
    }
    println!("shutting down Components");
    dispose_all(&fibers).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_event;

    #[test]
    fn console_event_keeps_its_payload_after_the_first_space() {
        assert_eq!(
            parse_event("ping hello world"),
            Some(("ping", "hello world"))
        );
        assert_eq!(parse_event("ping"), Some(("ping", "")));
        assert_eq!(parse_event("   \t"), None);
    }

    #[test]
    fn quit_is_not_an_event_protocol_exception() {
        assert_eq!(parse_event("quit"), Some(("quit", "")));
    }
}
