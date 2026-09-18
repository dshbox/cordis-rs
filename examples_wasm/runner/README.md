# Cordis Wasm runner

The runner is the native host for example Components. It creates one Cordis
`Context`, loads every `.wasm` file in a directory in lexical order, and spawns
one `ComponentPlugin` generation per file. Type `quit` (or close stdin) for
runner-controlled, reverse-order teardown.

> This is a local inspection tool. It accepts arbitrary local `.wasm` files;
> it has no signature/provenance policy or memory limiter. The narrow WIT host
> surface does not make it a production sandbox or deployment loader.

```sh
# Default directory: examples_wasm/modules
cargo run -p cordis-wasm-runner

# Or choose a directory explicitly
cargo run -p cordis-wasm-runner -- path/to/modules
```

An optional sibling `<module>.config` is passed as immutable bytes to that
module's `configuration.get` import. The runner owns Context routing, lifecycle
and shutdown; it does not grant Components a Loader, timer, scope, or Service
API.

After boot, each input line is one unscoped host Event: `<event-name> <UTF-8
payload>`. For example, with `hello_plugin` loaded, enter `ping world` to get
its `hello, world` diagnostic. The runner prints guest diagnostics through its
own console exporter. Empty lines are ignored; EOF starts the same reverse-order
teardown. `quit` is reserved by the runner; all other lines use the normal Event
grammar.
