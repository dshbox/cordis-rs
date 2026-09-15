# Migrating to Cordis v3

Cordis v3 first shipped on the `cordis-rs 0.7.x` line and currently publishes on
`0.8.x`. It is an architectural replacement, not a source-compatible update of
`0.6.x`. The old implementation remains maintained on `legacy/0.6` for critical
bug and security fixes.

## Current dependency identity

Applications keep the historical package and Rust import names:

```toml
cordis-rs = "0.8"
```

```rust
use cordis::Context;
```

The `cordis-rs` package is a thin facade over `cordis-core = "0.3"`. Framework
and plugin crates should normally depend on `cordis-core` directly. Timer and
loader capabilities are explicit optional crates rather than facade features:

```toml
cordis-timer = "0.3"
cordis-loader = "0.3"
```

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
