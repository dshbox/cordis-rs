# Application teardown

This page is application-author guidance. The normative lifecycle and ownership
contracts remain in [`docs/v3-public-interface.md`](v3-public-interface.md) and
ADRs [0028](adr/0028-fiber-generations-own-cleanup-runtime-owns-residency.md)
and [0029](adr/0029-lifecycle-commits-complete-and-critical-sections-are-closed.md).

## The short version

Cordis deliberately has no Runtime-wide shutdown operation. An application owns
its teardown policy:

1. retain every delivered `FiberHandle` whose Fiber the application intends to
   end;
2. on normal shutdown, call `dispose().await` for those handles;
3. when order matters, compose that order in the application; the demonstrated
   Harness/Roster policy is reverse spawn order; and
4. attempt every planned disposal instead of stopping after the first refusal.

`FiberHandle::dispose().await` is the deterministic lifecycle end for one
non-root Fiber. Return means its terminal barrier has completed: the current
generation is closed and drained, `Disposed` has been published, and the exact
Registry residency occurrence has been unlinked.

There is no equivalent `Runtime::shutdown()`, and dropping a `Context`,
`LoadOutcome`, or `FiberHandle` is not a substitute for lifecycle teardown.

## A demonstrated application policy

The repository's consumer-of-record Harness records delivered handles in spawn
order and tears them down in reverse order:

```rust
use cordis::{FiberHandle, FiberState};

async fn teardown(fibers: &[FiberHandle]) {
    for fiber in fibers.iter().rev() {
        if fiber.state() == FiberState::Disposed {
            continue;
        }
        if let Err(error) = fiber.dispose().await {
            eprintln!("teardown refused for {}: {error}", fiber.name());
            // Attempt the remaining Fibers.
        }
    }
}
```

That order is **consumer composition policy**, not hidden parenthood in core.
Spawning a Fiber records provenance but creates no parent/child lifecycle
ownership. A spawned Fiber can outlive the Fiber whose Context initiated the
spawn. Another application may choose a different explicit order when its own
domain requires one.

The important properties demonstrated by
[`examples/common/tests/boot.rs`](../examples/common/tests/boot.rs) are:

- `teardown_disposes_in_reverse_spawn_order_attempt_all` — reverse spawn-order
  disposal reaches every ordinary Fiber;
- `teardown_attempts_later_handles_after_one_dispose_refusal` — one
  `LifecycleRecursion` refusal does not stop later planned disposals;
- `teardown_is_idempotent_across_repeat_calls` — the shared helper is safe to
  invoke repeatedly; and
- `roster_push_returns_the_same_handle_it_records` plus
  `roster_holds_spawn_order_across_a_mid_flow_report` — the Roster retains the
  actual delivered lifecycle controls in spawn order.

`LifecycleRecursion` is a refusal to self-wait from the Fiber's own settle
dependency graph. Ordinary top-level application teardown should normally be
outside that graph, but attempt-all policy keeps one refusal from suppressing
cleanup of unrelated Fibers.

## What `dispose()` completes

A Fiber generation owns the cleanup obligations admitted through its apply
`Context`: Service publications, listener/exporter registrations, timer
operations, `Context::run` tasks, and effects. Terminal disposal closes the
generation gate and drains what that generation still owns.

The drain is deterministic where the contract promises ordering:

- cleanup obligations are claimed in strict reverse commit order;
- cleanup runs sequentially and attempt-all;
- a cleanup returned error or contained panic is secondary diagnostic
  information and does not stop the drain; and
- joined generation-owned tasks finish before the lifecycle barrier completes.

`FiberState::Disposed` is therefore not published before terminal cleanup
finishes. `dispose().await` waits through that full barrier; it is not merely a
request to begin cleanup.

Restart and committed update also replace a generation and drain the old
generation before their own documented barrier, but they do not end the Fiber.
Application shutdown uses terminal `dispose()` for the Fibers the application
owns.

## Keep delivered handles

A successful `Context::spawn()` hands the caller a `FiberHandle`; Registry
residency keeps the Fiber alive independently of how many handle clones remain.
Dropping every handle does not dispose the resident Fiber.

Loader follows the same handoff rule. Before `LoadPlan::load` returns, Loader
owns already-obtained handles and rolls them back if result handoff is
abandoned. After a `LoadOutcome` is delivered, dropping the outcome is inert:
the caller owns the delivered handles and should retain the ones needed for
later teardown. `LoadOutcome::fiber_handles()` is the canonical way to enumerate
those delivered controls.

## Root Context and root registrations

The root Fiber/generation lives for the Runtime's duration. Dropping root
`Context` clones does not close that generation, and Cordis does not implicitly
drain it when the last application handle disappears.

If application code installs a root-owned listener, exporter, effect, or other
exactly controlled registration that must end before process exit, retain and
use that registration's explicit remove/disarm/dispose control where available.
Do not rely on `Context` Drop to clean root-owned registrations.

Cordis guarantees framework-owned completion after the documented commit points
while the process remains alive; it does not promise a process-exit drain.
Applications that need orderly shutdown should therefore run their explicit
Fiber teardown before terminating the process.
