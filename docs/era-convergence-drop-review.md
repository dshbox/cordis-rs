# Era convergence last-Arc destruction review

Baseline: `origin/main` `a86c2bb` (after PR #179 and release PR #180). Scope:
`era_swap()`'s three affected-dependent convergence passes and the undelivered
successor cleanup. User-owned `Plugin::Input` destructors may panic in `Drop`.

## The seam

`converge()` awaited `FiberHandle::new(fiber).ready()` per captured dependent
and dropped the temporary handle at the end of each loop statement. A
dependent disposed while the era transaction was in flight is unlinked from
the Registry, so that handle can hold the Fiber's **last strong reference**:
its destruction runs the sealed `Plugin::Input` destructor synchronously on
the convergence owner's stack (`Fiber` has no manual `Drop`; the compiler
drop of `spawn_state` reaches `TypedPlugin::state`). `spawn_completion` and
`detach` add no panic boundary of their own — an escaped destructor panic
killed the era replacement owner, its oneshot sender died with the task, and
the `era_swap` caller met
`expect("era replacement owner retains its completion sender")`. On the
undelivered-successor cleanup the same loop aborted mid-pass, leaving every
later captured dependent unconverged, and the cleanup's trailing successor
handle drop carried the same hazard for the change's Input.

## Reachability per path

All three interleavings below are driven entirely through the public API
(`Context::new`, `spawn`, `provide`, `effect`, `dispose`, `era_swap`,
`add_exporter`, task cancellation). Determinism uses test-only scheduling
probes at the two destruction seams — the `DRIFT_KICK_PROBE` /
`READY_FAILURE_PROBE` house pattern — plus `Weak::strong_count()` barriers
proving the captured Arc is the last strong reference before the destructor
is armed. The probes park the owning *task* on a `Notify` — never a
completion-runtime worker thread — and are keyed by FiberId in per-test
slots, so concurrently running probe regressions can neither starve the
shared completion runtime nor overwrite each other's handshakes. The probes
park the framework between a barrier observation and a drop; they never
fabricate state.

| Path | Capture | Public-API window for dispose + handle drop | Regression |
| --- | --- | --- | --- |
| Entry convergence | `entry_dependents` snapshot in precommit, before the completion owner starts | The owner parks in the source's terminal drain for as long as a user-registered effect cleanup stays pending; any other task can `dispose().await` a dependent and drop its public handle there. No probe needed: the user cleanup itself holds the barrier open, and the entry snapshot's Arc is proven last by count before arming. | `entry_convergence_contains_a_disposed_dependent_last_arc` |
| Final convergence | fresh `capture_dependents` query after successor visibility | `ready()` awaits each captured dependent; its owner can complete disposal and drop the handle while the pass is between that dependent's `ready()` return and its drop. The probe parks exactly there, with a skip counter letting the earlier entry pass (same dependent, still live, unarmed) run through. | `final_convergence_contains_a_disposed_dependent_last_arc` |
| Undelivered-successor cleanup | fresh `capture_dependents` after the guard's successor disposal; the undelivered successor itself never reached a caller | The one public route: cancel the `era_swap` caller after the owner committed — the owner's send fails, the handoff guard detaches the cleanup, and the trailing successor drop destroys the change's Input. The successor's apply parks on the change input's own gate; the cleanup probe parks between the cleanup's convergence barrier and the trailing drop, after the successor's disposal and every transient owner clone completed, so the parked handle is provably the last Arc. The cleanup's convergence loop can also meet a disposed dependent's armed Input destructor mid-loop: its regression skips the probe through the entry and final passes (the dependent is alive and unarmed there) and parks the cleanup pass at the dependent's drop; while that probe is parked, the successor's strong count is proven down to the cleanup's single remaining handle (the disposal's transient owner clone waited out), so the survivor — spawned after it on the same single edge (capture preserves spawn order), with its own unload parked in a user cleanup gate — holds that count at exactly one until the barrier passes: releasing the gate, and only that, lets the loop finish and the successor's trailing drop destroy it. | `cancelled_swap_cleanup_contains_the_successor_input_destruction` and `undelivered_cleanup_converges_past_a_dependent_destructor_panic` |

Each regression is discriminating: with the containment removed, the entry
and final regressions fail through the owner panicking on the
`cordis-completion` thread and the caller losing its completion sender, the
cancelled-swap regression fails through the successor's destructor panic
escaping the detached cleanup with no routed diagnostic, and the
loop-continuation regression fails the same way — the dependent's panic
escapes the cleanup's convergence loop, nothing is routed, and the loop's
remaining work (the survivor's convergence observation and the successor's
trailing drop) never runs. Barrier assertions are made synchronously right
after `era_swap()` returns — before any `ready()` call, since `ready()`
itself drives a pending committed recheck and would mask a skipped final
barrier — and survivor states in the cleanup regressions are read the same
way. Report-count assertions discriminate the pass: the final regression's
single report proves the armed destruction happened in the final pass, not
the entry pass that ran unarmed.

Post-fix, each destructor is destroyed under its own containment boundary
(`contained::contain`, the PR #179 pattern) with the affected Fiber's logger
channel, so attached exporters receive exactly one warn diagnostic per
destructor panic and the barrier continues.

## Other paths examined

| Path | Last user-owned reference at risk? | Boundary |
| --- | --- | --- |
| `run_committed_replacement` source clones | The era-swapped source's seal is emptied by `into_successor` (or its panic is already caught at that seam), so destroying the source runs no user Input destructor. | Caller-owned handle drops stay on the caller's stack. |
| Precommit `retained_snapshot` | Requires `DependencyIndex::complete == false`, reachable only through `disable_for_test()` on this baseline. | Documented in `lifecycle-arc-drop-review.md`; not publicly reachable. |
| `converge` future dropped mid-loop | The entry/final passes run on the completion runtime, which is never cancelled; the cleanup runs under `detach`/`DetachedWork`, which transfers rather than drops a pending future. | Not publicly reachable. |
| `Fiber::dispose`'s detached owner clone | A disposed Fiber's transient last-Arc drop can land on that detached task instead of any era seam, for every dispose caller. | A separate generic-disposal seam, not era-specific and not reachable through era code alone; recorded here as an adjacent finding for a future audit, not fixed by this change. The cancelled-swap regression absorbs its window in-test (the probe parks after the clone's death); production code keeps the documented race. |

This is a selected public interleaving, not proof of all schedules. The
containment change covers every destructor the era transaction itself
releases; framework invariant panics for impossible private states keep
their existing policy.
