# cordis examples

The examples suite is core's consumer of record (ADR 0002,
no-consumer-no-API): core API surface lands only with an exercising
example.

Promise, binding on every seat: each example runs standalone via
`cargo run -p <name>`, needs no TTY, and exits 0. The chat capstone is
fully scripted as well: its two frontend Plugins drive the scenario without
reading stdin.

## Consumer paths

| crate | demonstrated path |
|---|---|
| `hello_plugin` | typed preparation, spawn, Event notification and explicit teardown |
| `gateway` | JSON plan boot, exact Services, scoped waterfall/query and timeout outcomes |
| `worker_daemon` | dependency failure/healing, generation-owned tasks, sleep/interval and explicit drain |
| `scopes_tenants` | opaque exact Service placement independent of Event Scope routing |
| `logging_exporters` | Logger filtering/buffering, exact exporter removal, Runtime observation/snapshot and typed Plugin removal |
| `chat_capstone` | scripted frontends, typed update policy, era replacement and dependent convergence |

`examples/common` is the shared helper crate (`publish = false`):
the Roster's delivered-handle recording/report/teardown mechanics and the shared
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
best-effort Runtime observations and current snapshots), and finish with
`cargo run -p chat_capstone` — the capstone, which composes the whole
surface into one app with disposable frontends. All six consumer paths are
available; the examples-runnable CI gate runs the hardcoded
`cargo run -p` list above.
