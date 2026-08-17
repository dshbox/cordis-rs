# cordis-cli

Command-line runner for the [cordis-rs](https://crates.io/crates/cordis-rs)
plugin framework: `cordis run <config.yml>`.

Builds on [cordis-loader](https://crates.io/crates/cordis-loader) to give a
config-driven cordis application a process model:

```console
$ cordis run cordis.yml
cordis: worker ready (2 entries, config: cordis.yml)
```

- **daemon / worker** — `cordis run` supervises a worker subprocess that
  boots the loader. The worker exits with code `51` to request a hot
  restart and `52` to quit; only `51` respawns.
- **signals** — `SIGINT` / `SIGTERM` dispose the root context gracefully
  and exit `52`, so Ctrl+C stops the whole application cleanly.
- **dotenv** — `.env` and `.env.local` are loaded from the working
  directory at startup (existing environment variables always win), and
  `${{ env.NAME }}` templates in the config expand from there.
- **hot reload** — the entry file is watched (debounced); external edits
  reconcile fibers through the loader's diff machinery.

Plugins stop or restart the process through the `worker` service:

```rust
# use cordis::Context;
# fn main() -> cordis::Result<()> {
let handle = ctx.require::<cordis_cli::worker::WorkerHandle>("worker")?;
handle.restart(); // full worker reload (exit 51)
# Ok(())
# }
```

## Scope

The stock binary registers only the built-in `group` plugin; entries with
other names are recorded in the loader's `last_error()` and skipped.
Embedding applications register their own plugins via
[`cordis_loader::PluginRegistry`](https://docs.rs/cordis-loader) or fork
this runner; dynamic library loading (`dynamic` feature) is future work.
