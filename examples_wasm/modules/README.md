# Runner module directory

Place Component artifacts directly in this directory. The runner loads every
`*.wasm` file in lexical order.

For example, build the guest then copy its artifact here:

```sh
cargo build --manifest-path examples_wasm/hello_plugin/Cargo.toml --target wasm32-wasip2
cp examples_wasm/hello_plugin/target/wasm32-wasip2/debug/cordis_wasm_hello_plugin.wasm \
  examples_wasm/modules/hello.wasm
cargo run -p cordis-wasm-runner
```

`hello.config` beside `hello.wasm` becomes the raw immutable value returned by
the Component's `configuration.get` import. Do not commit build artifacts to
this directory; they are deployment inputs.
