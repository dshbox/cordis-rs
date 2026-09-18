# `logging_exporters` Component

This extraction does not port exporter registration into Wasm. It emits one
diagnostic at each Cordis level during activation and one during disposal. The
runner (or a host integration test) owns filtering, buffering, channel policy
and Runtime observation.

```sh
cargo build --manifest-path examples_wasm/logging_exporters/Cargo.toml --target wasm32-wasip2
cp examples_wasm/logging_exporters/target/wasm32-wasip2/debug/cordis_wasm_logging_exporters.wasm \
  examples_wasm/modules/logging_exporters.wasm
cargo run -p cordis-wasm-runner
```
