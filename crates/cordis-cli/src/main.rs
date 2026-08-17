//! The `cordis` binary: see [`cordis_cli`] for the actual implementation.

fn main() {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    std::process::exit(cordis_cli::run(args));
}
