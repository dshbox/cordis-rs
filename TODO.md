# Pending review findings

Deferred items from the 2026-08 strict review. None block a release; each
needs a decision before acting (API break, upstream-parity question, or
test-only nit).

## API design

- `EventsService::clear()` is global but hangs off a per-context service: any
  context can wipe every hook in the root, bypassing fiber ownership. Either
  remove it, mark it `#[doc(hidden)]`, or scope it to the calling fiber's
  hooks.
- `CordisError::context` prepends context to the message, the opposite of
  `anyhow::Context` (attach a source). The name misleads Rust users; a rename
  such as `prefixed_with` would be honest but is a breaking change.
- ~~`Fiber::await_ready` and `Fiber::dispose_async` are transparent
  pass-throughs to their synchronous versions. They exist for upstream
  signature parity but imply cancellable/async semantics that do not exist.~~
  Resolved 2026-08-18 (issue #34): both methods now document loudly that they
  are synchronous pass-throughs that never suspend or yield.
- `Value` (and thus `Config`) has no `PartialEq`/`Display`; plugin config
  comparison in `update_value` relies on `Arc` pointer identity (`ptr_eq`).
  Two structurally equal configs are treated as different allocations.
  Resolution (2026-08-17): keep as-is. The loader stack passes
  `cordis_include::Node` as config, which carries structural `PartialEq`,
  so entry-level config diffing never depends on `Value` identity.

## Behavior gaps (parity questions)

- `ReflectService::set_value` replaces a service value without advancing its
  generation or notifying injecting fibers (documented since the review).
  Resolution (2026-08-17): verified against upstream 4.x
  (`vendor/cordis/src/reflect.ts`, `ReflectService.set`): upstream also only
  assigns `impl.value = value` with no re-evaluation. Keep the current
  semantics and the manual `notify()` escape hatch; the loader wraps
  entry-level `inject` into plugin `Inject` declarations, so service
  add/remove already reconciles dependents through the core fiber machinery.

## Test hygiene

- ~~`src/effect.rs` tests use bare `.lock().unwrap()` (lines ~270, ~276)
  instead of the poison-tolerant `utils::lock` used everywhere else.~~
  Fixed 2026-08-17 alongside the `Fiber::idle` addition.
