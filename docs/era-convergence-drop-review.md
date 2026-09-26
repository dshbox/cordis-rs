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
later captured dependent unconverged.

## Reachability per path

| Path | Capture | Public-API window for dispose + handle drop | Regression |
| --- | --- | --- | --- |
| Entry convergence (`run_committed_replacement`) | `entry_dependents` snapshot in precommit, before the completion owner starts | The owner parks in the source's terminal drain for as long as a user-registered effect cleanup stays pending; any other task can `dispose().await` a dependent and drop its public handle there | `entry_convergence_contains_a_disposed_dependent_last_arc` |
| Final convergence (`converge_final`) | fresh `capture_dependents` query after successor visibility | `ready()` awaits each captured dependent's settle; the dependent's owner can complete its disposal and drop the handle while that await is parked | same shared seam; the entry regression pins the loop, and its survivor dependent asserts the final barrier (re-apply against the successor's publication) |
| Undelivered-successor cleanup (`cleanup_undelivered_successor`) | fresh `capture_dependents` after the guard's successor disposal | same shared seam for bystander dependents, plus the trailing `successor` handle drop at the cleanup's tail — the undelivered successor never reached a caller, so that handle can be the change's Input last Arc | `undelivered_successor_cleanup_contains_the_successor_input_destruction` |

Both regressions drive only public API (`Context::new`, `spawn`, `provide`,
`effect`, `dispose`, `era_swap`, `add_exporter`). Determinism comes from the
scheduling seam itself — the source's registered cleanup parks the terminal
barrier on a `Notify` — plus `Weak::strong_count()` barriers proving the
captured Arc is the last strong reference before the destructor is armed.
The second regression fully disposes the successor before offering it, so
the guard's cleanup meets an already-complete terminal barrier and its
trailing drop is provably the last reference with no transient owner clone
in flight.

Pre-fix, the entry regression fails with the owner panicking on the
`cordis-completion` thread and the caller losing its completion sender; the
undelivered regression fails with the successor's destructor panic escaping
the detached cleanup and no routed diagnostic. Post-fix, each destructor is
destroyed under its own containment boundary (`contained::contain`, the PR
#179 pattern) with the affected Fiber's logger channel, so attached
exporters receive exactly one warn diagnostic per destructor panic.

## Other paths examined

| Path | Last user-owned reference at risk? | Boundary |
| --- | --- | --- |
| `run_committed_replacement` source clones | The era-swapped source's seal is emptied by `into_successor` (or its panic is already caught at that seam), so destroying the source runs no user Input destructor. | Caller-owned handle drops stay on the caller's stack. |
| Precommit `retained_snapshot` | Requires `DependencyIndex::complete == false`, reachable only through `disable_for_test()` on this baseline. | Documented in `lifecycle-arc-drop-review.md`; not publicly reachable. |
| `converge` future dropped mid-loop | The entry/final passes run on the completion runtime, which is never cancelled; the cleanup runs under `detach`/`DetachedWork`, which transfers rather than drops a pending future. | Not publicly reachable. |
| `Fiber::dispose`'s detached owner clone | A disposed Fiber's transient last-Arc drop can land on that detached task instead of any era seam, for every dispose caller. | A separate generic-disposal seam, not era-specific and not reachable through era code alone; recorded here as an adjacent finding for a future audit, not fixed by this change. |

This is a selected public interleaving, not proof of all schedules. The
containment change covers every destructor the era transaction itself
releases; framework invariant panics for impossible private states keep
their existing policy.
