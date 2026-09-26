# Detached disposal owner and last-Arc destruction review

Baseline: `origin/main` `4dc5ef4d9823cf11027a625e543fbad6fc60d4ac` (PR #181 and release PR #182 merged). Scope: the `Arc<Fiber>` captured by ordinary `Fiber::dispose()`'s detached owner, and a panicking user `Plugin::Input::Drop`. This review does not change framework invariant panic policy.

## Public API reachability

A caller can `Context::spawn(PreparedPlugin::from_input(plugin, input))`, start `FiberHandle::dispose()`, and cancel the caller after the terminal claim while a registered cleanup is pending. The public handle can then be dropped. Once the owner finishes cleanup and `release_residency()` removes the Registry's references, the detached owner's `Arc<Fiber>` can be the last strong reference. The retained `SpawnState::Stored.plugin` owns a `TypedPlugin` whose state retains `Plugin::Input`; final destruction of that Fiber can therefore run arbitrary `Input::Drop`, including a panic. No private state mutation is needed. Other strong references (a surviving public handle, another snapshot, or a caller still polling `dispose()`) defer that destructor to their own drop site; they do not make the detached-owner schedule impossible.

The pending `DetachedWork` transfers its pinned future on ordinary executor shutdown. Its `transfer_on_drop` flag is cleared while polling and a poll unwind is not transferred. Neither `detach` nor the discarded Tokio join handle contains or reports a user destructor panic through Cordis's logger. A last-Arc panic on the owner therefore produces an unobserved Tokio task failure/panic-hook diagnostic.

## Barrier order and effect

1. `dispose()` claims the slot and closes the terminal gate before calling `effect::detach`. The owner holds its own strong `fiber` reference across `complete_claimed_dispose().await`.
2. `complete_claimed_dispose()` runs `teardown_body()` inside `settle_ctx::bracket`: it marks the Fiber dead, unregisters dependency edges, runs the whole cleanup drain, publishes `Disposed`, and clears its error. It then abandons the slot. The bracket's allocation pin is gone when the await returns.
3. `release_residency()` removes the exact Registry residency and conditionally prunes its allocation before `publish_dispose_complete()` stores the completion flag and notifies waiters. The owner reference remains alive throughout these operations. Any temporary Registry reference destroyed during release cannot be the last strong reference while the owner is alive.
4. Only after `publish_dispose_complete()` returns can the owner future complete and release its captured `fiber`. For the on-runtime path, `DetachedWork::poll` drops the completed inner future in `work.take()`; for the completion-runtime path, Tokio drops the completed task future. Thus an `Input::Drop` caused specifically by the detached owner's final reference is **after** the terminal barrier. The owner has no subsequent cleanup, slot release, or result send.

A waiter that still holds a `FiberHandle` can finish `dispose()` and observe the complete barrier; its strong reference prevents the owner from being the final reference until the waiter releases it. A racing `ready()` sees `Disposed` after the slot goes idle. Framework-owned bulk removal awaits each `dispose()`; this owner-final-drop panic cannot interrupt its subsequent members, because its awaiting member reference is still alive until that await returns. Other last-reference paths on a caller or snapshot are separate destruction seams and are not covered by this conclusion.

## Decision boundary

The owner-final-reference panic is reachable through public API but occurs after cleanup, `Disposed`, exact unlink, and waiter notification. The remaining issue is loss of a Cordis logger diagnostic for that destructor panic, not a stranded terminal barrier or skipped later cleanup. No containment or recovery logic is added for this seam. This ordering argument is specific to the ordinary `Fiber::dispose()` owner on this baseline; it does not prove every arbitrary user destructor in `teardown_body()`, `release_residency()`, or another lifecycle owner's captured values safe.
