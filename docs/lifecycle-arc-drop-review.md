# Lifecycle slot and last-Arc destruction review

Baseline: `origin/main` `35cd4cb` (after PR #177 and release PR #178). Scope: safe public API execution while a Fiber holds its lifecycle slot; user-owned `Plugin`, `Plugin::Input`, Service, exporter, and callback captures may panic in `Drop`.

## Reachable exit: Service transition's dependent snapshot

`Fiber::transition()` runs during a provider's committed restart, update, dispose, initial spawn, or convergence while it still owns its lifecycle slot. On an Active boundary, `ServiceStore::commit_fiber_transition()` takes the complete dependency index snapshot and commits each affected dependent's recheck revision under the Service lock. It returns a `CommittedServiceDrift` with strong `Arc<Fiber>` references. `drift.kick()` runs after releasing that lock, but **before the provider releases its lifecycle slot**.

Another caller can finish `dependent.dispose()` and drop its public handle after the snapshot. Registry residency and the dependent's lifecycle owner then release their references. The strong reference in `affected` becomes the last one. The old `for fiber in affected` released it after `kick_committed_recheck()` without containment; dropping the Fiber destroys its stored `Plugin::Input`. A panicking destructor unwound the provider's committed restart owner before its final slot release and completion send. The caller observed a missing sender, and later `ready()` could remain Pending.

The regression `disposed_dependent_last_arc_does_not_orphan_provider_restart` uses only public spawn, restart, and dispose operations to create the race. A test-only scheduling probe pauses after the snapshot, and a `Weak` count confirms the snapshot holds the last strong reference before resuming. With the containment removed, the test fails with a panicked completion owner and a missing restart sender; with it, the provider restarts, `ready()` returns Active, and another dependent still converges. Each snapshot reference is destroyed under its own containment boundary, outside Service synchronization. Panic reports use the affected Fiber's logger channel so attached exporters receive one warn diagnostic; exporter dispatch contains its own callbacks and destructor panics. The fallback Registry snapshot receives the same treatment so a future production fallback will retain this property.

## Other paths examined

| Path | Last user-owned reference while slot held? | Boundary |
| --- | --- | --- |
| `DependencyIndex::dependents_of()` redundant upgraded references | A duplicate retains a corresponding Arc in its deduplicated result. Its redundant Arc cannot be the final reference inside the index scan. | Index only stores `Weak`; no user destructor under its lock. |
| `era_swap()` entry snapshot | A dependent returned by the complete index remains in `entry_dependents`; the source remains strongly owned by the caller. The Registry fallback snapshot requires `DependencyIndex::complete == false`, which only `disable_for_test()` can set on this baseline. | The early recursion and Closed exits release the source slot before returning owned values. The private fallback's last-Arc case does not establish public reachability. |
| `settle_once()` and `StateLease` | A lease returns the state to the seal during apply, and the seal stays installed on the live Fiber; apply/poll and lease unwinding are within `catch_contained()`. | Returned error conversion and panic-payload destruction are contained. |
| Update and era replacement | Old input destruction in `update_pass()` occurs after starting the committed owner; `into_successor()` in the era owner is caught and completes the final dependent barrier before returning its panic. | Existing owner regressions cover both. |
| Cleanup and logger exporter | A claimed cleanup closure and its captures are consumed in `execute_cleanup()` containment. Logger snapshot references are released individually under containment. | Existing public exporter regression covers self-removal and last-Arc `Drop`. |

This is a selected public interleaving, not proof of all schedules. Framework invariant panics for impossible private slot states retain their existing policy; this change contains only a reachable user destructor. The test probe controls scheduling and is compiled only in the crate's unit-test build.
