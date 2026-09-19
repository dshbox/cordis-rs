# cordis examples

The examples suite is core's consumer of record (ADR 0002,
no-consumer-no-API): core API surface lands only with an exercising
example.

Promise, binding on every seat: each example runs standalone via
`cargo run -p <name>`, needs no TTY, and exits 0. The chat capstone is
fully scripted as well: its two frontend Plugins drive the scenario without
reading stdin.

## Seats

| crate | seat | lands with |
|---|---|---|
| `hello_plugin` | on-ramp: smallest correct program | landed (core-v2 ticket 22) |
| `gateway` | declarative JSON boot + four-flip SRE runbook; timeout shape's seat | landed (core-v2 ticket 23) |
| `worker_daemon` | crash-loop / heal / drain; `Context::run` pump; sleep + interval shapes' seat | landed (core-v2 ticket 24) |
| `scopes_tenants` | tour: sole consumer of the isolate surface | landed (core-v2 ticket 25) |
| `logging_exporters` | tour: sole consumer of emit/observe + exporters | landed (core-v2 ticket 26) |
| `chat_capstone` | dual-frontend chat; the suite's capstone | landed (core-v2 ticket 27) |

`examples/common` is the shared helper crate (`publish = false`):
boot/teardown mechanics (`boot_report` / `teardown`) and the shared
ops-console section rendering (`section`).
Policy — supervision, error aggregation, narration ordering — stays
app-side. For the application-facing shutdown pattern demonstrated by that
helper — retain delivered `FiberHandle`s, reverse-spawn-order `dispose().await`,
attempt-all, and no implicit Runtime shutdown — see
[`docs/application-teardown.md`](../docs/application-teardown.md).

Start here: `cargo run -p hello_plugin`, then `cargo run -p gateway`
for the declarative boot + runbook seat, then `cargo run -p worker_daemon`
for the fiber-lifecycle-under-failure seat, then
`cargo run -p scopes_tenants` for the tenant-realms tour, then
`cargo run -p logging_exporters` for the observation tour (exporters,
`internal/*` narration, the record seams), and finish with
`cargo run -p chat_capstone` — the capstone, which composes the whole
surface into one app with disposable frontends. All six seats have
landed; the examples-runnable CI gate runs the hardcoded
`cargo run -p` list above.
