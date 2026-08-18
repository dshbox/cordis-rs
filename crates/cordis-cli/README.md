# cordis-cli

**English** | [简体中文](README.zh-CN.md)

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
  restart, `52` to quit, and `53` when the loader never booted; only `51`
  respawns.
- **signals** — `SIGINT` / `SIGTERM` dispose the root context gracefully
  and exit `52`, so Ctrl+C stops the whole application cleanly. A signal
  that reaches only the daemon (`kill <pid>`, supervisors with per-process
  kill modes) is forwarded: the daemon closes the worker's stdin pipe, the
  worker tears down through the same graceful path, and one that ignores
  it for ten seconds is killed — so the worker also exits when the daemon
  dies for any reason.
- **dotenv** — `.env` and `.env.local` are loaded from the working
  directory at startup (existing environment variables always win), and
  `${{ env.NAME }}` templates in the config expand from there.
- **hot reload** — the entry file is watched (debounced); external edits
  reconcile fibers through the loader's diff machinery.

Plugins stop or restart the process through the `worker` service:

```rust,ignore
// inside a plugin's apply, with `ctx` the plugin context:
let handle = ctx.require::<cordis_cli::worker::WorkerHandle>("worker")?;
handle.restart(); // full worker reload (exit 51)
```

## Scope

The stock binary registers only the built-in `group` plugin; entries with
other names are recorded in the loader's `last_error()` and skipped,
unless `--plugin-dir <dir>` (repeatable) lets them resolve from
dynamic-library plugins fingerprint-checked against the running toolchain —
a changed library hot-restarts the worker. Embedding applications register
their own plugins via
[`cordis_loader::PluginRegistry`](https://docs.rs/cordis-loader) or fork
this runner.
