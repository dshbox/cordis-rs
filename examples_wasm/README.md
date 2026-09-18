# WASM component examples

This directory contains Component extractions from the native `examples/`
suite. It is deliberately not a one-for-one mirror: the native examples remain
the reference for `cordis-core`, and a Runtime mechanism does not become a
guest capability merely to make a port look symmetrical.

The current WIT world exposes lifecycle, immutable configuration, host-owned
diagnostics, and declared opaque Events. `hello_plugin` is the first complete
guest port. The remaining examples retain their Runtime orchestration in the
host and move only their plugin behavior to Components.

## Extraction map

| Native example | Component extraction | Boundary decision |
| --- | --- | --- |
| `hello_plugin` | `hello_plugin/`: declared `ping` handler | Complete; Context dispatch and Fiber teardown remain host-owned |
| `scopes_tenants` | None | Scope and ServiceRealm placement are Runtime behavior |
| `logging_exporters` | `logging_exporters/`: guest diagnostics | Complete; exporters and Runtime observation remain Runtime-wide authority |
| `worker_daemon` | Planned timer callback behavior | Scheduler remains host-owned; Services are out of scope |
| `gateway` | Planned request/reply route behavior | Loader plan and update policy are host control-plane behavior |
| `chat_capstone` | Planned request/reply frontend behavior | Scope/Service topology remains host-owned |

## Rules

- Each guest is a `wasm32-wasip2` Component and has no direct access to
  `cordis-core`.
- All host authority remains in `cordis-wasm`; guest imports are narrow,
  capability-oriented WIT interfaces.
- A port must demonstrate its behavior from guest code. A native host-side
  reimplementation is not a port.
- Every guest instance still maps to exactly one Cordis apply generation.
- A native example whose subject is Runtime topology remains native. It is not
  a failed port and it does not justify a broad guest import.

Configuration and diagnostics are host-assigned. Events cross the boundary
only when the guest declared a subscription in its manifest; Runtime
mechanisms do not become guest imports.

See [BOUNDARY.md](BOUNDARY.md) for the planned request/reply and timer
capabilities, and for the explicit Services non-goal.
