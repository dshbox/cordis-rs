# Component boundary

The Component is a guest-owned behavior generation, not a serialized Cordis
`Context`. Its imports remain operation-shaped and host mediated.

## Toolchain boundary

The spike pins Wasmtime `48.0.0` with `wit-bindgen 0.62.0`. This is not a
performance selection: Wasmtime 36 was tested first because it declared a
lower Rust requirement, but it could not consume Components generated from the
repository's asynchronous WIT (`async func`) bindings. The selected non-RC
pair is validated end-to-end here, and it currently exceeds Cordis's
workspace MSRV. It remains experimental until the author accepts that toolchain
boundary or chooses to isolate `cordis-wasm` from the published workspace.

| Capability | Guest can do | Host retains |
| --- | --- | --- |
| Event | emit and handle declared notifications | routing, registration ownership, teardown |
| request/reply | send bytes and return/decline bytes | query ordering, responders, cancellation |
| timer | name a one-shot or periodic deadline; receive `tick(id)` | scheduling, task ownership, cancellation |
| diagnostics | emit a record | channel identity and exporters |

## Services are intentionally out of scope

Native `Service<T>` transports an exact Rust value in an opaque, realm-mapped
slot. A generic WIT replacement would either pretend arbitrary `T` values are
portable or create a string-keyed data bus that bypasses Cordis's service and
scope semantics. Neither is honest.

`scopes_tenants` therefore remains native for now: its point is proving
`ServiceRealm` placement. `worker_daemon` can extract timer-driven behavior
only after a host-owned permit policy is supplied outside the guest boundary.

If a real application later needs a cross-Component value, it must introduce a
named, versioned application protocol with explicit authority and lifecycle
rules. It must not extend this standard plugin ABI with `get-service` or
`publish-service`.
