# API freeze conformance evidence

This is a non-normative contract-to-production review. Production source baseline:
`ed07d3149a5711541eaabd34633122e6eb3c52bd`, fetched on 2026-09-28. The delivery
spec is [#200](https://github.com/dshbox/cordis-rs/issues/200). Each path's completion
record identifies its fixed review point, resulting commit, two review axes and
eight local gates. A final candidate recommendation is a separate delivery.

Authority remains the [glossary](../CONTEXT.md), [architecture](v3-architecture.md),
[public inventory](v3-public-interface.md), accepted ADRs and
[parity ledger](v3-upstream-parity-ledger.md). The
[compatibility policy](compatibility-policy.md) separately owns downstream and
published-sibling promises. Existing [failure](failure-boundary-audit.md) and
[concurrency](lifecycle-concurrency-modeling.md) records retain their own baselines
and bounded coverage; their historical action language is not a current defect queue.

Covered means a named test distinguishes the stated contract from the nearest
rival behavior. Evidence insufficient means the cited material cannot justify
the claim; it does not by itself establish a defect. Known deviation identifies
an actual mismatch and its disposition. Accepted boundary names a stronger
guarantee the authority declines. N/A requires a reason. Source review and
examples complement discriminating tests; none establish universal correctness
or unbounded progress.

## Prepare, seal and spawn

Delivery: [#201](https://github.com/dshbox/cordis-rs/issues/201).
Contract: [authoring/facades](v3-public-interface.md#authoring-style-and-macro-surface),
[preparation and creation](v3-public-interface.md#plugin-preparation-sealing-and-creation),
ADR [0029](adr/0029-lifecycle-commits-complete-and-critical-sections-are-closed.md),
[0038](adr/0038-plugin-input-names-role-prepared-wrappers-name-stage.md) and
[0039](adr/0039-consumer-fiber-control-is-a-fiber-handle.md).
Production: [typed sealing](../crates/cordis-core/src/plugin.rs),
[creation transaction](../crates/cordis-core/src/fiber/spawn.rs),
[core facade](../crates/cordis-core/src/lib.rs),
[application facade](../crates/cordis/src/lib.rs).

| Status / scenario | Discriminating evidence | Nearest rival excluded |
| --- | --- | --- |
| Covered: preparation fails or panics before admission | [configuration.rs](../crates/cordis-core/tests/configuration.rs): `plugin_preparation_failure_and_panic_precede_all_runtime_state`, `declaration_panics_unwind_before_lifecycle_admission` | A typed error/panic admits a Fiber or is converted into a lifecycle failure |
| Covered: sealing materializes declarations once | configuration.rs: `sealing_materializes_declarations_once_before_lifecycle`, `inject_overlay_replaces_or_clears_one_normalized_service_row` | Lifecycle reevaluates name/inject or an overlay adds a second effective declaration |
| Covered: minimal operation-specific bounds and stage capabilities | [configuration UI suite](../crates/cordis-core/tests/configuration_ui.rs): pass `ui-configuration/pass/minimal_bounds.rs`; fail `prepared_plugin_wrong_pair.rs`, `prepared_change_wrong_pair.rs`, `prepared_values_are_move_only.rs`, `prepared_values_are_must_use.rs` in its fail directory | Minimal bounds rejects extra universal Clone/Sync/Default requirements; wrong-pair fixtures reject mismatched typed associations; move-only/must-use fixtures reject reuse or ignored prepared stages |
| Covered: quiescent Active/Pending handoff | [spawn.rs](../crates/cordis-core/tests/spawn.rs): `eligible_spawn_delivers_a_live_quiescent_active_fiber_handle`, `missing_requirements_hand_off_a_stable_pending_fiber_handle_without_applying`, `a_mutation_racing_the_initial_apply_is_converged_before_handoff` | Handoff precedes current-target convergence or Pending runs apply |
| Covered: initial failure has no attempted resident | spawn.rs: `initial_apply_error_rolls_back_lifo_and_leaves_no_resident_fiber`, `initial_apply_panic_rolls_back_and_leaves_no_resident_fiber` | Apply failure leaks residency or skips/reorders rollback |
| Covered: cancellation changes ownership at creation commit | spawn.rs: `caller_cancellation_before_commit_allocates_nothing`, `caller_cancellation_after_commit_completes_disposal_and_unlink`, `off_runtime_cancellation_drives_the_rollback_under_a_runtime` | Precommit cancellation changes framework state, or postcommit cancellation strands admitted work |
| Covered with private scheduling aid: framework invalidation before handoff | [spawn production-module test](../crates/cordis-core/src/fiber/spawn.rs): `handoff_barrier_discriminates_invalidation_from_caller_cancellation` | Interrupted denotes caller cancellation, or invalidation after successful handoff retroactively prevents delivery |
| Covered: supported facade and retired spellings | [core facade UI](../crates/cordis-core/tests/facade_ui.rs): pass `ui-facade58/pass/canonical_paths.rs`; fail `ui-facade58/fail/removed_facade_paths.rs`, `root_whitelist_is_exact.rs` | Approved semantic paths fail to compile, or retired/source-layout and extra root paths remain usable |
| Covered: sibling feature boundary | [feature-unification consumer probes](../ci/internal-api-contract.sh): `default-core` must fail on __internal, `sibling-unification` must compile direct-core __internal use, `facade-boundary` must fail on cordis::__internal; [application facade UI](../crates/cordis/tests/facade_ui.rs): `internal_core_seams_do_not_escape_the_application_facade` | Internal details escape through cordis, default core exposes them, or real direct-core sibling unification is falsely said to be unreachable |

The Interrupted discriminator pauses a yield-free handoff window using a private
probe, then invokes public typed removal. It verifies a reachable safe-API race,
not a public probe contract. Initial spawn tests and UI fixtures exercise ordinary
public capabilities; compiler snapshots are compared on the repository's pinned
Rust patch and rust-src, not floating stable.

Accepted boundaries: direct synchronous Plugin preparation/declaration panic
unwinds before lifecycle admission; `from_input` establishes type association,
not producing-object identity; handle Drop is inert. `internal-api` is unsupported
downstream even when leaf feature unification exposes `__internal` to direct core
users. [ADR 0040](adr/0040-published-sibling-seams-are-compatibility-obligations.md)
still requires compatible published sibling packages to compile against that seam.

Known deviation resolved in this path: the architecture reading table named
eleven ADRs through 0038 while its actual index included twelve through 0039.
The table/count and ADR navigation now agree, with compatibility ADR 0040 linked
separately. No public declaration, behavior, glossary term or accepted decision
was changed. No material evidence gap or production defect was identified in
this path; its bounded execution coverage does not prove every scheduling history.

Consumer composition: [guide rule 1](consumer-guide.md#1-prepare-input-seal-it-then-spawn-and-retain-the-handle)
and the headless [hello_plugin](../examples/hello_plugin/src/main.rs) example.
