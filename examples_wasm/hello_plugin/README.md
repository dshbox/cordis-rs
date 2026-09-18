# `hello_plugin` Component

This is the guest port of `examples/hello_plugin`.  It declares one `ping`
subscription in `manifest.describe`; when the host dispatches a `HostEvent`
named `ping` with a UTF-8 payload, it emits `hello, <payload>` through the
Fiber-assigned diagnostics channel.

Build the Component with:

```sh
cargo build --manifest-path examples_wasm/hello_plugin/Cargo.toml --target wasm32-wasip2
```

The host must instantiate it through `ComponentPlugin`, emit
`HostEvent::new("ping", b"world")` in the owning Context, and dispose the
Fiber. Dispatching after disposal reaches no guest handler.  The host retains
the listener registration, Context routing, and lifecycle teardown; this
Component only owns the Echo behavior.
