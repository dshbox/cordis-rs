# Migrating to Cordis v3

Cordis v3 first shipped on the `cordis-rs 0.7.x` line and currently publishes on
`0.10.x`. It is an architectural replacement, not a source-compatible update of
`0.6.x`. The old implementation remains maintained on `legacy/0.6` for critical
bug and security fixes.

## Current dependency identity

Applications keep the historical package and Rust import names:

```toml
cordis-rs = "0.10"
```

```rust
use cordis::Context;
```

The `cordis-rs` package is a thin facade over `cordis-core = "0.5"`. Framework
and plugin crates should normally depend on `cordis-core` directly. Timer and
loader capabilities are explicit optional crates rather than facade features:

```toml
cordis-timer = "0.5"
cordis-loader = "0.5"
```

## From facade 0.10.x / semantic 0.5.x to 0.11.x / 0.6.x

Update `cordis-rs` dependency requirements to `0.11` and any direct `cordis-core`,
`cordis-timer` and `cordis-loader` requirements together to `0.6`. The facade
re-exports core types; a `cordis::Context` backed by core `0.6` is a different
Rust type from a direct core `0.5` `Context`, so cross-crate values must use the
same core line.

Public vocabularies that are expected to grow are now `#[non_exhaustive]`, so a
later release can add a variant or a record field without a breaking change.
Downstream code that matched them exhaustively needs two mechanical edits:

- Add a wildcard arm (`_ => ...`) to every `match` on these enums:
  `LifecycleOperation`, `PluginFailureKind`, `EffectFailureKind`,
  `InvocationFailureKind`, `DispatchOutcomeKind`, `EventOperation`,
  `ListenerRole`, `Routing`, `RuntimeObservation`, `ObservationRouting`,
  `ResolverFailureKind`, `EntryOutcome` and `RealmPolicy`.
- Add `..` to every pattern on these record variants:
  `RuntimeObservation::{FiberResidency, FiberState, ServiceVisibility,
  ListenerRegistration, DispatchCompleted}` and
  `EntryOutcome::{Group, Disabled, Pruned, Spawned, Failed}`. These variants can
  no longer be constructed outside their defining crate. Downstream tests that
  fabricated them must obtain real ones from a Runtime: `RuntimeObservation`
  records from observers registered with `Context::observe_runtime`, and
  `EntryOutcome` rows from `LoadPlan::load`.

Constructing `Routing` and `RealmPolicy` values is unaffected, and
`RealmPolicy::Shared { label }` keeps its fields. `FiberState`, `FiberRole`,
`UpdateOutcome`, `QueryOutcome`, `ResidencyChange`, `ListenerChange`,
`InjectEntry` and `TimeoutOutcome` stay exhaustive.

Loader source rows `PluginEntry` and `EntryGroup` are now `#[non_exhaustive]`, so
a source field can later be added without another breaking release. Struct
literals no longer compile outside `cordis-loader` (`E0639`). Construct rows with
`PluginEntry::new(config)` and `EntryGroup::new(name)`, then assign the fields
that differ; the fields stay `pub`:

```rust
// before
let entry = PluginEntry {
    key: Some("worker".into()),
    name: None,
    config: serde_json::Value::Null,
    disabled: false,
    inject: Vec::new(),
    isolate: Vec::new(),
};
let group = EntryGroup { name: "root".into() };

// after
let mut entry = PluginEntry::new(serde_json::Value::Null);
entry.key = Some("worker".into());
let group = EntryGroup::new("root");
```

`PluginEntry::new(config)` leaves `key` and `name` unset, the entry enabled and
`inject` and `isolate` empty, which equals deserializing `{"config": ..}`. The
JSON wire format, serde attributes and strict unknown-field rejection are
unchanged, and code that deserializes rows needs no change. `IsolateEntry` stays
a plain struct with public fields.

**Narrowed promise: creation after its commit.** As with
[ADR 0041](docs/adr/0041-user-destructor-panics-are-best-effort.md), no API
spelling or signature changes, but a documented contract is narrowed.
`Context::spawn` used to promise that, after its allocation commit, the framework
completes the creation independently of caller polling. That was never
implemented: creation up to FiberHandle delivery is driven by polling the spawn
future. The contract now says so. While a committed spawn future is alive, the
creation advances only as that future is polled, and work that waits on the new
Fiber, such as `remove_plugins` of its allocation, waits too. Dropping the future
still hands the creation to framework completion, which disposes the undelivered
Fiber. `LoadPlan::load` spawns inside its own future, so a load future kept but
no longer polled holds its in-progress entry and rolls nothing back. Poll such a
future to completion or drop it; do not park it, for example as the unfinished
half of a `select`. See
[ADR 0029](docs/adr/0029-lifecycle-commits-complete-and-critical-sections-are-closed.md)
and the [creation law](docs/v3-public-interface.md#plugin-preparation-sealing-and-creation).

Committed `FiberHandle::dispose()` and `Context::remove_plugins` now keep the
stronger promise again: once their owner first waits, it runs without the
committing caller, so a future kept but no longer polled no longer stalls them or
the `dispose()` and `ready()` calls that coalesce onto them. Await these futures;
do not block a Tokio worker thread on them (for example with
`futures::executor::block_on` inside a task), because the handed-off owner may
be queued on that same worker.

## From facade 0.9.x / semantic 0.4.x to 0.10.x / 0.5.x

Update `cordis-rs` dependency requirements to `0.10` and any direct `cordis-core`,
`cordis-timer` and `cordis-loader` requirements together to `0.5`. The facade
re-exports core types; a `cordis::Context` backed by core `0.5` is a different
Rust type from a direct core `0.4` `Context`, so cross-crate values must use the
same core line.

This line changes no API spelling, signature or runtime behavior. It narrows a
documented contract: values supplied to Cordis must not panic when dropped, and
Cordis contains and reports such a panic only on a best-effort basis
([ADR 0041](docs/adr/0041-user-destructor-panics-are-best-effort.md), the
[user destructor rule](docs/v3-public-interface.md#user-destructor-panics)). The
earlier promises for Event callback, snapshot, answer, tail and continuation
destruction and for era-swap `Plugin::Input` destruction are withdrawn. Which
operation reports such a panic, whether an associated value is discarded and
whether the panic resumes in an awaiting caller are unspecified. Code that relied
on those outcomes should make the affected `Drop` implementations panic-free. See
the [core changelog](crates/cordis-core/CHANGELOG.md) and
[facade changelog](crates/cordis/CHANGELOG.md).

## From facade 0.8.x / semantic 0.3.x to 0.9.x / 0.4.x

Update `cordis-rs` dependency requirements to `0.9` and any direct `cordis-core`,
`cordis-timer` and `cordis-loader` requirements together to `0.4`. The facade
re-exports core types; a `cordis::Context` backed by core `0.4` is a different
Rust type from a direct core `0.3` `Context`. The package/import names and API
spellings remain unchanged, but cross-crate values must use the same core line.

Loader `0.4` rejects unknown source-schema fields, including fields belonging
to another tagged variant. Correct misspelled keys and remove extra fields;
put Plugin-specific data in the Plugin entry's `config`. A field such as `label`
on `private` or `config` on `required` is now a parse error. `Serialize` output
is unchanged. See the [Loader changelog](crates/cordis-loader/CHANGELOG.md) and
[facade changelog](crates/cordis/CHANGELOG.md) for the coordinated release notes.

## From 0.7.x to 0.8.x

`0.8.x` removes the old `Fork` spelling in favor of `FiberHandle`. There is no
compatibility alias. Update the public type paths and Loader outcome spellings:

```text
cordis::Fork                         -> cordis::FiberHandle
cordis::lifecycle::Fork              -> cordis::lifecycle::FiberHandle
EntryOutcome::Spawned { fork, .. }   -> EntryOutcome::Spawned { fiber_handle, .. }
LoadOutcome::forks()                 -> LoadOutcome::fiber_handles()
```

This is a naming break, not a lifecycle-ownership change: dropping a
`FiberHandle`, including the last clone, still does not dispose the Fiber. See
[ADR 0039](docs/adr/0039-consumer-fiber-control-is-a-fiber-handle.md) for the
naming decision and [`docs/v3-migration.md`](docs/v3-migration.md) for the full
lifecycle migration inventory.

## From 0.6.x to v3

The migration is governed by the accepted v3 ADRs in `docs/adr/0028` through
`0039`. The most visible architectural changes are:

- Context no longer implies one hidden hierarchy: Service isolation, Event Scope,
  and intercept are orthogonal axes.
- Service dependencies converge on exact `(Service, ServiceRealm)` publication
  assignments; there is no implicit fallback lookup.
- Events use typed contracts, explicit routing, semantic listener roles, and
  completion-aware occurrence claiming.
- Generation cleanup ownership is distinct from Runtime/Registry Fiber residency.
- update preserves Fiber identity; era swap is an identity-breaking replacement.
- update control is a precommit lifecycle protocol, not an ordinary dispatchable Event.
- runtime observation follows committed protocol truth and never drives it.
- public modules and crate seams expose semantic roles rather than internal stores,
  registry topology, or other representation details.

## Legacy companion crates

The legacy `cordis-include`, `cordis-group`, and `cordis-cli` crates are not
mechanically carried into v3. They remain part of the `0.6.x` ecosystem until a
v3-native design is justified by the new semantic architecture.

For the exhaustive migration inventory, see [`docs/v3-migration.md`](docs/v3-migration.md).
For the exact v3 public surface, see
[`docs/v3-public-interface.md`](docs/v3-public-interface.md).
