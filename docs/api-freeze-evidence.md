# API freeze conformance evidence

This is a non-normative contract-to-production review. Production source baseline:
`680b3b058659a6318e51ec174537a424118f2927`, fetched on 2026-09-29. The original
2026-09-28 deliveries reviewed `ed07d31`; their receipts are historical, not
validation of this later revision. The PR #210 follow-up rechecks affected claims
after #214, #216, #217 and #218 landed. The delivery spec is [#200](https://github.com/dshbox/cordis-rs/issues/200). Each path's completion
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

## Findings since the original review

The original negative finding was incomplete: later review found reachable rivals
that the original selected tests did not exclude. This record supersedes that
finding, preserves the discovered history and cites the added discriminators.

| Finding / disposition | Authority and rechecked evidence | Remaining boundary |
| --- | --- | --- |
| F1: stale-transient ready panic, identified → fixed → reviewed in [#214](https://github.com/dshbox/cordis-rs/pull/214) | Public ready/restart race arranged by a private scheduling probe; current state-publication sequence forces retry after an intervening publication | One deterministic interleaving, not every schedule; no public probe API |
| F2: committed work stranded on an idle current_thread origin, identified → fixed → reviewed in [#216](https://github.com/dshbox/cordis-rs/pull/216) | ADR 0029; public dispose, typed removal and abandoned creation regressions leave the origin alive but idle | Framework completion still needs the process and completion executor to run; it cannot keep an earlier external driver alive |
| F3: ignored unknown Loader source fields, identified → accepted behavior change → implemented and reviewed in [#217](https://github.com/dshbox/cordis-rs/pull/217) | Interface/source schema and Loader changelog now explicitly require strict Deserialize; public parse/load controls distinguish typos from valid disabled/private rows | This is a deliberate pre-1.0 Deserialize compatibility change, not a demonstrated violation of the former unspecified unknown-field contract; arbitrary Plugin config remains governed by its own schema |
| F4: wait_state deadline thread cost and spawn-refusal panic, identified → fixed → reviewed in [#218](https://github.com/dshbox/cordis-rs/pull/218) | Shared monotonic scheduler, cancellation/removal and contained wakes; public thread-count/waker tests plus isolated refusal injection | DeadlineUnavailable is a new variant on a non-exhaustive enum; private injection proves the scheduler refusal/retry seam, while production maps that refusal to the public error |
| F5: apply placement disclosure, identified → documented in this follow-up | ADR 0029, Plugin rustdoc, interface and guide rule 1 now state non-blocking polls and shared completion-runtime placement | Ordinary spawn's pre-handoff drift rechecks remain caller-driven; no origin-runtime affinity, dedicated per-Fiber executor or progress guarantee while consumers block both completion workers; earlier captured resources retain their original driver and the caller's paused clock does not control completion-runtime time |

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
| Covered: abandoned creation completes with an idle origin | [idle_origin_runtime.rs](../crates/cordis-core/tests/idle_origin_runtime.rs): `abandoned_creation_is_rolled_back_while_the_origin_current_thread_runtime_is_idle` | Rollback requires another poll or shutdown of the origin runtime |
| Covered with private scheduling aid: framework invalidation before handoff | [spawn production-module test](../crates/cordis-core/src/fiber/spawn.rs): `handoff_barrier_discriminates_invalidation_from_caller_cancellation` | Interrupted denotes caller cancellation, or invalidation after successful handoff retroactively prevents delivery |
| Covered: supported facade and retired spellings | [core facade UI](../crates/cordis-core/tests/facade_ui.rs): pass `ui-facade58/pass/canonical_paths.rs`; fail `ui-facade58/fail/removed_facade_paths.rs`, `root_whitelist_is_exact.rs` | Approved semantic paths fail to compile, or retired/source-layout and extra root paths remain usable |
| Covered: sibling feature boundary | [feature-unification consumer probes](../ci/internal-api-contract.sh): `default-core` must fail on __internal, `sibling-unification` must compile direct-core __internal use, `facade-boundary` must fail on cordis::__internal; [application facade UI](../crates/cordis/tests/facade_ui.rs): `internal_core_seams_do_not_escape_the_application_facade` | Internal details escape through cordis, default core exposes them, or real direct-core sibling unification is falsely said to be unreachable |


Source-reviewed apply placement: [initial settle and later passes](../crates/cordis-core/src/fiber/inertia.rs),
[era successor](../crates/cordis-core/src/fiber/era.rs) and
[completion runtime](../crates/cordis-core/src/effect.rs) establish the F5 disclosure.
Ordinary spawn polls initial settlement inline, including pre-handoff drift
rechecks. Restart/update/background convergence and era-successor initial apply
run on the shared two-worker completion runtime. Blocking both workers can stall
unrelated lifecycle work and async cleanup. This source review establishes current
placement, not a new permanent worker-count promise or an unbounded-progress proof.
F5 discloses an execution-location/progress boundary under non-blocking apply polls;
it does not establish a framework invariant violation when Plugin code blocks
the workers. Historical saturation observation at `ed07d31`: on an eight-worker caller Tokio
runtime, two Plugins were spawned, then restarted. Their restart applies each
blocked a completion worker on a synchronous condition-variable gate. After both
had entered the gate, an unrelated Fiber with a trivial `effect(|| async {})`
cleanup was disposed. Its dispose barrier did not complete within a two-second
caller-runtime timeout. The gate was then opened, both restarts were joined and
all Fibers were explicitly disposed. The probe's assertion expected disposal
within that bound and therefore failed intentionally; it characterized saturation,
not a supported non-blocking Plugin failing its lifecycle contract.

The original observation used Rust 1.95.0 and dependencies vendored from upstream
tags, rather than the canonical Rust 1.98.1 and locked registry artifacts. It is
not a current-head gate result, a universal progress proof or normal-suite
conformance coverage. Current apply placement is separately source-reviewed above;
the freeze recommendation requires its own exact-head validation.

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
was changed by that navigation repair. F2's abandoned-creation defect was later
identified and fixed in #216; the idle-origin discriminator now covers it. No
unresolved defect in this path is currently identified; bounded execution coverage
does not prove every scheduling history.

Consumer composition: [guide rule 1](consumer-guide.md#1-prepare-input-seal-it-then-spawn-and-retain-the-handle)
and the headless [hello_plugin](../examples/hello_plugin/src/main.rs) example.

## Context axes and exact Services

Delivery: [#202](https://github.com/dshbox/cordis-rs/issues/202).
Contract: [axes](v3-public-interface.md#context-and-its-axes),
[configuration](v3-public-interface.md#dependency-declarations-and-service-configuration),
[publication](v3-public-interface.md#service-publication-and-lookup), ADRs
[0031](adr/0031-service-convergence-tracks-exact-publication-assignments.md) and
[0032](adr/0032-context-axes-are-orthogonal.md).
Production: [Context](../crates/cordis-core/src/context.rs),
[Service occurrences](../crates/cordis-core/src/service.rs),
[settlement](../crates/cordis-core/src/fiber/inertia.rs),
[dependency projection](../crates/cordis-core/src/deps.rs).

| Status / scenario | Discriminating evidence | Nearest rival excluded |
| --- | --- | --- |
| Covered: realm identity and atomic validation | [realms.rs](../crates/cordis-core/tests/realms.rs): `realms_are_opaque_runtime_local_placement_identities`, `explicit_equal_realms_join_only_the_mapped_service_slot`, `fresh_private_derivations_never_rendezvous`, `realm_batch_rejects_duplicates_before_foreign_realms` | Cross-Runtime/textual rendezvous, mapping all Services to one shared realm, or partial mutation before a later validation failure |
| Covered: view equivalence and axis independence | realms.rs: `root_resets_fiber_isolate_scope_and_intercept_in_the_same_runtime`, `clone_preserves_fiber_realm_scope_and_intercept_exactly`, `isolate_derivation_preserves_event_reachability`; [context_equivalence.rs](../crates/cordis-core/tests/context_equivalence.rs): `equivalent_views_match_for_service_lookup_and_event_routing`, `equivalent_views_register_cleanup_to_the_same_current_fiber` | Derivation history changes lookup/routing/ownership or root creates a different Runtime |
| Covered: synchronous typed composition and declaration normalization | [configuration.rs](../crates/cordis-core/tests/configuration.rs): `service_preparation_and_composition_are_synchronous_and_typed`, `service_composition_can_reenter_context_configuration`, `inject_normalization_erases_order_and_duplicates_from_dependency_targets`, `inject_overlay_cannot_select_a_service_realm`, `intercept_derivation_changes_no_other_context_axis` | Framework merge/default, user composition under a lock, declaration-order identity, overlay realm control or intercept altering another axis |
| Covered: exact lookup/publication and Loading visibility | [service_v3.rs](../crates/cordis-core/tests/service_v3.rs): `exact_lookup_distinguishes_unavailable_contract_mismatch_and_realms`, `duplicate_and_contract_mismatch_publication_refuse_without_disturbing_current_occurrence`, `loading_occupation_is_invisible_until_active_then_visibility_drifts` | Fallback lookup, failed publication damaging the current occurrence, or Loading values being visible |
| Covered: occurrence control and durable visibility drift | service_v3.rs: `exact_publication_set_preserves_target_remove_commits_drift_and_drop_is_inert`, `off_runtime_visibility_commit_is_driven_by_later_ready`, `close_withdraws_before_cleanup_and_stale_cleanup_cannot_remove_replacement`, `closed_current_reports_mutation_closed_but_replacement_makes_old_handle_stale_first`, `successful_set_and_remove_destroy_outgoing_values_outside_service_synchronization` | Payload set restarts dependents, Drop withdraws publication, off-runtime mutation is lost, old cleanup removes replacement, or user Drop runs under synchronization |
| Covered: exact fixed edges, missing projection and failure parking | service_v3.rs: `fixed_exact_dependency_edges_survive_restart_and_update`, `pending_missing_is_the_normalized_missing_projection`, `failed_target_parking_uses_exact_publication_and_explicit_restart_bypasses_it` | Restart/update remaps prerequisites, Pending reports unrelated slots, or failure retries on payload-only set |
| Covered at private implementation seam: projection is not authority | [Context unit tests](../crates/cordis-core/src/context.rs): `incomplete_dependency_projection_falls_back_to_fiber_owned_edges`, `disabled_projection_preserves_service_settlement`; [deps.rs](../crates/cordis-core/src/deps.rs): `clearing_and_rebuilding_projection_changes_no_authoritative_edges` | A disabled, partial or rebuilt index loses authoritative edges and dependent convergence |

The projection tests privately arrange index damage and then use public
spawn/provide/ready behavior. They prove the production fallback under those
arrangements, not a public index-disable API or all possible corruption histories.
Accepted boundaries: axes are independent views, not ownership/authorization;
exact realm lookup has no fallback; same-occurrence set preserves SemanticTarget;
restart/update retain exact edges. Era replacement resolves fresh edges from its
captured immutable recipe; it need not select different slots. No material
evidence gap, known deviation or production defect was identified in this path.
Usage: [guide rules 2–3](consumer-guide.md#2-choose-service-placement-event-reachability-and-configuration-independently)
and [scopes_tenants](../examples/scopes_tenants/src/main.rs).



## Lifecycle control and era replacement

Delivery: [#203](https://github.com/dshbox/cordis-rs/issues/203).
Contract: [lifecycle](v3-public-interface.md#fiber-identity-lifecycle-and-typed-group-removal),
[typed update](v3-public-interface.md#typed-update-control), ADRs
[0030](adr/0030-era-replacement-ends-one-fiber-before-creating-another.md) and
[0034](adr/0034-update-control-is-precommit-and-non-dispatchable.md).
Production: [Fiber control](../crates/cordis-core/src/fiber/mod.rs),
[precommit policy](../crates/cordis-core/src/update.rs),
[era owner](../crates/cordis-core/src/fiber/era.rs).

| Status / scenario | Discriminating evidence | Nearest rival excluded |
| --- | --- | --- |
| Covered: current-target ready versus passive wait | [lifecycle_v3.rs](../crates/cordis-core/tests/lifecycle_v3.rs): `ready_reports_typed_current_failure_and_drives_a_new_target`, `wait_state_is_passive_for_off_runtime_drift`, `failed_is_published_only_after_rollback_finishes` | Old-target failure after drift, wait driving settlement, or Failed preceding rollback |
| Covered with private scheduling aid: stale transient ready observation | [Fiber unit test](../crates/cordis-core/src/fiber/mod.rs): `ready_transient_state_racing_complete_restart_never_panics` | A completed public restart leaves ready matching an obsolete Loading state and panicking after its idle recheck |
| Covered: shared wait deadlines and caller-waker containment | [wait_state_threads.rs](../crates/cordis-core/tests/wait_state_threads.rs): `many_pending_waits_share_one_deadline_thread`; [wait_state_waker_panic.rs](../crates/cordis-core/tests/wait_state_waker_panic.rs): `a_panicking_waker_does_not_stop_other_deadlines` | Each pending wait holds an OS thread, Duration::MAX expires, or one caller wake stops unrelated deadlines |
| Covered at private scheduler seam: thread refusal and exact arm retirement | [deadline.rs](../crates/cordis-core/src/deadline.rs): `refused_scheduler_thread_is_reported_and_retried`, `cancelled_deadlines_leave_no_entries`, `unrepresentable_deadline_never_elapses_and_needs_no_scheduler`, `a_lost_worker_is_replaced_for_already_armed_deadlines` | Thread refusal panics or fabricates Elapsed, cancellation leaves entries, unrepresentable timeout elapses, or worker loss strands already-armed deadlines |
| Covered: retry preserves identity and committed completion | [lifecycle_barriers.rs](../crates/cordis-core/tests/lifecycle_barriers.rs): `restart_preserves_identity_and_retries_a_same_target_failure`, `cancelled_precommit_restart_waiter_does_not_replace_the_generation`, `cancelled_postcommit_restart_waiter_does_not_stop_the_restart`, `committed_restart_survives_origin_runtime_shutdown` | Retry allocating a new Fiber, precommit cancellation mutating generation, or postcommit completion depending on waiter/origin executor |
| Covered: provisional control, mismatch, veto and admission revalidation | [update_control.rs](../crates/cordis-core/tests/update_control.rs): `wrong_contract_is_precommit_and_typed`, `around_can_veto_without_reaching_private_tail`, `private_tail_is_provisional_until_outer_control_returns`, `unrecovered_control_error_preserves_old_generation`, `close_during_awaited_control_reports_admission_lost`, `same_fiber_update_recursion_is_refused_before_control` | Tail committing early, veto/error changing generation, wrong contract reaching control, or awaited control skipping admission/recursion checks |
| Covered: forward-only committed candidate and Pending | update_control.rs: `accepted_update_can_commit_to_stable_pending_without_apply`, `postcommit_apply_failure_is_invisible_to_control_and_candidate_is_retained`, `postcommit_update_apply_panic_is_contained_and_keeps_the_new_input`, `mapper_panic_is_contained_precommit_and_preserves_the_old_generation` | Committed guaranteeing Active, outer policy catching postcommit failure, apply failure restoring old input, or mapper panic committing |
| Covered: cancellation and sealed policy capabilities | update_control.rs: `cancelling_during_precommit_control_commits_nothing`, `cancelling_postcommit_waiter_does_not_cancel_update_owner`; [lifecycle UI](../crates/cordis-core/tests/lifecycle_ui.rs): fail `ui-lifecycle/fail/update_rejects_raw_any.rs`, `update_observer_is_not_update_policy.rs`, `update_responder_is_not_update_policy.rs`, `removed_internal_update_event.rs`; [configuration UI](../crates/cordis-core/tests/configuration_ui.rs): fail `ui-configuration/fail/prepared_values_are_move_only.rs` | Caller cancellation stranding commit, erased input or notification/response adapters becoming policy, or candidate reuse being allowed |
| Covered: era source claim, death before birth and fresh identity | [era_replacement.rs](../crates/cordis-core/tests/era_replacement.rs): `replacement_breaks_identity_while_update_and_restart_do_not`, `old_terminal_cleanup_precedes_successor_apply`, `racing_replacements_claim_one_live_source_and_attempt_one_successor`, `successor_gets_sibling_scope_and_fresh_generation_resources_without_cascading_children`, `wrong_contract_refuses_before_source_claim` | Old/new overlap, two successors, identity reuse, child cascade or wrong-contract source destruction |
| Covered: final convergence and failed-successor completion | era_replacement.rs: `return_waits_for_dependents_to_converge_to_the_successors_current_publication`, `mid_swap_dependent_is_included_by_the_fresh_final_query`, `successor_apply_failure_keeps_primary_cause_cleans_successor_and_waits_final_dependents`, `successor_apply_panic_remains_the_primary_incomplete_cause_after_cleanup` | Intermediate-target handoff, missed new dependent, lost primary cause or resident failed successor |
| Covered: era cancellation ownership and immutable recipe | era_replacement.rs: `cancellation_while_waiting_for_source_claim_is_no_effect`, `cancellation_during_old_cleanup_cannot_stop_committed_replacement_completion`, `cancellation_during_successor_settle_cleans_the_undelivered_successor`, `replacement_replays_a_closed_spawn_origins_view_without_false_successor_lost`; update_control.rs: `era_swap_never_invokes_update_control` | Preclaim mutation, cancelled owner, leaked undelivered successor, incorrectly rejected captured view or update policy invoked for era |

Candidate consumption is a move-only compile contract plus consuming production
signatures for every refusal/result branch; runtime tests distinguish their
commit effects, not a fictional candidate-return channel. Accepted boundaries:
ready is quiescence, not universal Active; control cannot recover postcommit apply;
era uses its immutable captured recipe, with no replacement Context parameter;
Incomplete is forward-only. A changed-captured-mapping test is N/A because that
mutation is not publicly expressible. These bounded tests do not prove every
schedule. F1 and F4 were missed in the original review and are now fixed with
the discriminators above. WaitStateError now includes Elapsed, Recursion and
DeadlineUnavailable; existing non-exhaustive matching keeps its wildcard. The
private refusal injection is not a public OS-refusal fixture; source review checks
the production mapping to DeadlineUnavailable. No unresolved defect in this path
is currently identified.
Usage: [rule 4](consumer-guide.md#4-choose-same-fiber-control-or-replace-the-era-then-retain-the-right-handle)
and [chat_capstone](../examples/chat_capstone/src/main.rs).

## Generation ownership and consumer teardown

Delivery: [#204](https://github.com/dshbox/cordis-rs/issues/204).
Contract: [effects/tasks](v3-public-interface.md#effects-and-tasks),
[lifecycle/removal](v3-public-interface.md#fiber-identity-lifecycle-and-typed-group-removal),
[application teardown](application-teardown.md), ADRs
[0028](adr/0028-fiber-generations-own-cleanup-runtime-owns-residency.md) and
[0029](adr/0029-lifecycle-commits-complete-and-critical-sections-are-closed.md).
Production: [effects/completion](../crates/cordis-core/src/effect.rs),
[Fiber drain/tasks](../crates/cordis-core/src/fiber/mod.rs),
[attribution](../crates/cordis-core/src/fiber/settle_ctx.rs),
[Registry](../crates/cordis-core/src/registry.rs),
[Roster](../examples/common/src/lib.rs).

| Status / scenario | Discriminating evidence | Nearest rival excluded |
| --- | --- | --- |
| Covered: all retained core families share admission/ownership law | [generation_ownership.rs](../crates/cordis-core/tests/generation_ownership.rs): `loading_active_and_permanent_root_admit_and_own_their_core_resources`, `pending_failed_closing_and_disposed_refuse_every_core_retained_family`, `every_core_family_publish_versus_close_has_only_refusal_or_owned_commit`, `cross_fiber_use_does_not_transfer_owner_restart_replaces_and_spawn_is_not_parenthood` | Service, ordinary/update listeners, observers, exporters, effects or run tasks bypass the generation gate; published-but-unowned residue; use transfers ownership; origin disposal cascades |
| Covered: exact manual claim and cancellation boundary | [effects.rs](../crates/cordis-core/tests/effects.rs): `winning_dispose_completes_after_caller_cancellation`, `dispose_cancelled_before_the_claim_changes_nothing`, `winning_disarm_beats_an_in_flight_drain`, `stale_control_reports_false_after_the_drain_won`, `dispose_reports_returned_error_and_consumes_the_occurrence`, `dispose_reports_panic_and_consumes_the_occurrence` | Preclaim cancellation consumes cleanup, postclaim cancellation strands it, double execution, or failure restores an occurrence |
| Covered: sequential reverse-order attempt-all and task join | effects.rs: `effect_sync_holds_lifo_position_against_async_effects`, `cross_resource_cleanup_holds_reverse_commit_positions`, `failing_and_panicking_cleanups_do_not_block_the_drain`, `run_drain_joins_after_the_tasks_own_effects_lifo`, `run_drain_join_contains_a_panicking_task`, `run_task_polling_and_output_destruction_stay_outside_framework_locks` | Sync bypasses LIFO, cleanup runs concurrently/stops on failure, drain skips task join or user polling/Drop holds framework locks |
| Covered: exact attribution transfer/refusal/expiry | [settle_guard.rs](../crates/cordis-core/tests/settle_guard.rs): `attributed_spawn_from_apply_refuses_every_lifecycle_self_wait`, `manual_dispose_from_apply_keeps_settle_attribution_across_cleanup_task`, `external_manual_dispose_does_not_invent_settle_attribution`; [private settle_ctx tests](../crates/cordis-core/src/fiber/settle_ctx.rs): `transferred_attribution_expires_with_its_source_scope`, `scopes_start_clean_and_raw_tokio_spawn_does_not_inherit_attribution` | Attributed task self-wait deadlocks, detached cleanup loses live frame, external cleanup gains false refusal, expired frames remain active or raw Tokio spawn inherits attribution |
| Covered: origin executor loss and task refusal | effects.rs: `async_cleanup_timer_outlives_origin_runtime_shutdown`, `run_off_the_runtime_refuses_and_starts_nothing`, `run_on_a_disposed_fiber_refuses_and_starts_nothing`; [lifecycle_barriers.rs](../crates/cordis-core/tests/lifecycle_barriers.rs): `committed_dispose_survives_origin_runtime_shutdown`, `dropping_context_handles_never_runs_root_cleanup_and_surviving_runtime_keeps_it_claimable` | Committed cleanup binds to lost origin executor; refused run starts work; Context Drop silently drains root |
| Covered: committed completion with a live but idle origin | [idle_origin_runtime.rs](../crates/cordis-core/tests/idle_origin_runtime.rs): `committed_dispose_completes_while_the_origin_current_thread_runtime_is_idle`, `committed_group_removal_completes_while_the_origin_current_thread_runtime_is_idle`; [settle_guard.rs](../crates/cordis-core/tests/settle_guard.rs): `abandoned_dispose_is_joinable_from_another_apply_on_current_thread`, `abandoned_group_removal_is_joinable_from_another_apply_on_multi_thread` | Completion depends on origin polling/shutdown, or caller-driven ownership falsely transfers another apply's recursion attribution |
| Covered: residency and no parent cascade | [residency.rs](../crates/cordis-core/tests/residency.rs): `admitted_child_outlives_disposed_spawn_origin`, `admission_first_child_finishes_after_its_origin_is_disposed`, `dropping_every_fiber_handle_does_not_end_a_resident_fiber` | Origin owns spawned Fiber, origin disposal aborts admitted child creation, or handle Drop unlinks residency |
| Covered: typed removal freezes one allocation | [registry_removal.rs](../crates/cordis-core/tests/registry_removal.rs): `typed_removal_freezes_the_detached_allocation_and_repeated_absence_succeeds`, `cancelling_typed_removal_before_detach_leaves_the_allocation_live`, `detach_commits_removal_and_caller_cancellation_cannot_stop_the_frozen_drain`, `cleanup_failure_and_panic_do_not_stop_other_frozen_members`, `self_wait_recursion_is_refused_before_typed_group_detach`, `committed_removal_survives_runtime_shutdown` | Removal absorbs later allocation, changes state before detach, loses members on cancellation/failure, or recursion detaches before refusal |
| Covered: explicit consumer policy | [boot.rs](../examples/common/tests/boot.rs): `teardown_disposes_in_reverse_spawn_order_attempt_all`, `teardown_attempts_later_handles_after_one_dispose_refusal`, `teardown_is_idempotent_across_repeat_calls`, `roster_push_returns_the_same_handle_it_records`, `roster_holds_spawn_order_across_a_mid_flow_report` | First refusal stops unrelated cleanup, delivered control is replaced, or report/repeated teardown reorders/repeats disposal |

Private attribution tests directly exercise the production frame protocol;
public self-wait tests cover reachable apply/manual-cleanup task boundaries.
Timer admission is assessed in its leaf path. Accepted boundaries: no Runtime
shutdown, parent cascade, Registry inter-Fiber order or process-exit drain.
Framework completion cannot extend an earlier captured external IO/timer driver's
lifetime. `spawn_attributed` grants attribution, not cleanup ownership. Ordinary
Event/Timer future cancellation remains caller-owned; winning manual cleanup
claim instead transfers completion to the framework. F2 was missed by the
shutdown-only coverage and is now fixed with idle-origin discriminators. CallerDriven
polls committed runtime-agnostic owners inline; abandonment uses the multi-thread
origin when available, otherwise the completion runtime. No unresolved defect in
this path is currently identified. Usage: [rules 5–6](consumer-guide.md#5-register-resources-with-their-generation-and-separate-ownership-from-attribution)
and [worker_daemon](../examples/worker_daemon/src/main.rs).

## Event dispatch and exact claims

Delivery: [#205](https://github.com/dshbox/cordis-rs/issues/205).
Contract: [Events](v3-public-interface.md#event-contracts-roles-and-dispatch),
[errors](v3-public-interface.md#event-errors),
[ADR 0033](adr/0033-events-are-typed-completion-aware-and-occurrence-claimed.md).
Production: [dispatch](../crates/cordis-core/src/events/dispatch.rs),
[adapters](../crates/cordis-core/src/events/listener.rs),
[occurrence store](../crates/cordis-core/src/events/store.rs),
[types/errors](../crates/cordis-core/src/events/types.rs).

| Status / scenario | Discriminating evidence | Nearest rival excluded |
| --- | --- | --- |
| Covered: routing and preflight precede factories/claims | [event_roles.rs](../crates/cordis-core/tests/event_roles.rs): `scoped_routing_distinguishes_root_ancestor_self_sibling_descendant_and_global`, `foreign_scope_contract_and_role_preflight_run_before_factory_or_callback`, `callback_context_is_the_registration_context_and_derivations_preserve_scope`, `scope_skip_does_not_construct_state` | Foreign Scope claims, Scope means all scopes, callback receives dispatch Context, or skipped/preflight-failed work constructs state |
| Covered: notification completion and parallel full-set claim | [event_dispatch.rs](../crates/cordis-core/tests/event_dispatch.rs): `emit_is_ordered_awaited_fail_first_and_correlates_the_failure`, `emit_parallel_claims_the_complete_set_before_any_callback_can_remove_it`, `emit_parallel_starts_all_awaits_all_and_reports_failures_in_listener_order`, `emit_parallel_uses_parallel_even_for_one_failure` | Detached emit, incremental parallel claims, early return on first failure, completion-order reporting or single-error shape switch |
| Covered: query presence and failure correlation | event_dispatch.rs: `query_is_sequential_first_answer_fail_fast_and_owned_through_remove`, `query_fails_fast_with_exact_correlation`, `query_preserves_false_zero_empty_text_and_empty_collection_answers`, `returned_errors_future_panics_and_state_factory_panics_are_normalized_with_correlation` | Falsey answer treated as Miss, continued invocation after answer/error, removal revokes claim or panic loses identity |
| Covered: owned waterfall and derived query | [event_waterfall.rs](../crates/cordis-core/tests/event_waterfall.rs): `waterfall_is_an_owned_outer_to_inner_onion_and_veto_skips_tail`, `waterfall_transports_move_only_args_without_clone`, `waterfall_contains_tail_panic_and_outer_around_can_recover_downstream_failure`, `waterfall_query_uses_same_routing_and_resolver_sees_answer_miss_and_failure` | Wrong onion/order, universal Clone, mandatory tail runs after veto, tail panic escapes or derived query changes routing/result |
| Covered: once and exact removal/cancellation | [event_occurrences.rs](../crates/cordis-core/tests/event_occurrences.rs): `duplicate_occurrences_are_independent_and_registration_drop_is_inert`, `remove_before_snapshot_claim_skips_but_claim_before_remove_finishes_owned_work`, `cancelling_dispatch_before_once_claim_leaves_the_occurrence_registered`, `cancelling_dispatch_after_once_claim_does_not_restore_the_occurrence`, `registration_racing_generation_close_never_strands_an_occurrence` | Drop unregisters, duplicates collapse, remove revokes claimed work, once restores after cancellation or generation race strands ownership |
| Covered: completed invocation destructor containment | event_occurrences.rs: `parallel_once_listener_capture_destruction_does_not_skip_claimed_sibling`, `once_around_capture_destruction_is_correlated_invocation_failure`, `callback_drop_panic_discards_an_answer_even_if_its_drop_also_panics` | Last capture Drop panic skips claimed siblings, loses correlation or returns an answer despite destructor failure |
| Covered: early exits preserve primary result and completion | [event_early_return.rs](../crates/cordis-core/tests/event_early_return.rs): `emit_failure_survives_unclaimed_removed_listener_drop_and_reports_completion`, `query_answer_survives_unclaimed_removed_listener_drop_and_reports_completion`, `waterfall_skipped_mapper_drop_preserves_tail_result_and_completion` | Unclaimed snapshot Drop replaces error/answer/tail or suppresses DispatchCompleted |
| Covered: unused tail and each uncalled snapshot are contained | event_waterfall.rs: `mapper_failure_survives_uncalled_tail_destructor_and_publishes_completion`, `preflight_failure_survives_unused_tail_destructor_and_publishes_completion`; [event_waterfall_uncalled_chain.rs](../crates/cordis-core/tests/event_waterfall_uncalled_chain.rs): `mapper_error_contains_each_uncalled_listener_destructor` | Unused continuation Drop replaces primary error, containment covers only one capture, or completion narration is lost |

These destructor regressions are reachable through ordinary public dispatch,
removal and observation APIs; the recent #196 fix is already in the production
baseline. Known documentation deviation resolved: the specialist inventory
omitted exported `event::with_state` although its body contract described it.
Accepted boundary: operation futures remain caller-owned. Pending-callback
cancellation does not promise detached completion; completed-invocation Drop
containment does not extend to every cancellation, unclaimed-removal or
Around-owned Next destruction. No remaining material evidence gap or supported
production defect was found. Usage: [rule 7](consumer-guide.md#7-select-an-event-role-and-routing-then-await-the-intended-completion),
[gateway](../examples/gateway/src/main.rs), [chat_capstone](../examples/chat_capstone/src/main.rs).

## Runtime observations and Logger

Delivery: [#206](https://github.com/dshbox/cordis-rs/issues/206).
Contract: [observations/snapshots](v3-public-interface.md#runtime-snapshots-and-observations),
[Logger](v3-public-interface.md#logger),
[ADR 0035](adr/0035-runtime-observation-follows-protocol-truth.md).
Production: [observation hub/records](../crates/cordis-core/src/observation.rs),
[snapshot selection](../crates/cordis-core/src/context.rs),
[Logger/exporters](../crates/cordis-core/src/logger.rs).

| Status / scenario | Discriminating evidence | Nearest rival excluded |
| --- | --- | --- |
| Covered: detached parallel attempt-all and generation ownership | [runtime_observation_delivery.rs](../crates/cordis-core/tests/runtime_observation_delivery.rs): `blocked_observers_run_in_parallel_without_delaying_source_and_keep_attribution`, `failing_and_panicking_observers_do_not_veto_and_later_observers_are_attempted`, `missing_executor_drops_delivery_without_changing_listener_truth`, `registering_generation_close_stops_future_delivery` | Source waits/vetoes, one observer blocks later attempts, missing executor alters listener truth or closed generation receives future delivery |
| Covered with private record capture: five committed families and outcome vocabulary | [observation unit tests](../crates/cordis-core/src/observation.rs): `loading_install_is_not_visibility_but_active_and_unlink_are_committed_records`, `exact_listener_and_primitive_completion_use_semantic_vocabulary`, `restart_preserves_residency_while_era_swap_replaces_it_and_manual_withdraw_is_exact`, `all_primitive_operations_report_semantic_outcomes` | Loading occupation narrated as visible, listener/primitive facts conflated, restart changes residency or outcome vocabulary follows representation |
| Covered: current snapshot selection and delivery-gap recovery | [runtime_snapshot.rs](../crates/cordis-core/tests/runtime_snapshot.rs): `empty_runtime_snapshot_is_exactly_the_permanent_root`, `snapshot_is_flat_current_residency_with_per_record_semantics`, `loading_publication_is_current_but_invisible_then_becomes_visible`, `closed_generation_physical_service_row_is_not_current_snapshot_state`, `later_snapshot_recovers_current_state_after_an_unobserved_gap_only`; observation unit: `disposed_record_remains_visible_until_exact_residency_unlink` | Snapshot omits root/resident Disposed rows, exposes stale closed rows, conflates occupied/visible or recovers replay history rather than current state |
| Covered: severity, channel fallback and foundation availability | [logger.rs](../crates/cordis-core/tests/logger.rs): `level_order_is_semantic_low_to_high_severity`, `min_level_routes_per_channel_name`, `log_record_accessors_are_semantic`, `logging_does_not_create_service_visibility_or_missing_edges`, `logger_remains_foundation_available_across_generation_phases` | Numeric/storage ordering drives severity, None disables, fields expose mutable storage or Logger becomes a Service prerequisite |
| Covered: isolated reentrancy/panic and exact removal | logger.rs: `reentrant_exporter_does_not_recurse_and_later_exporter_is_attempted`, `reentrant_filter_does_not_recurse_through_logging`, `panicking_exporter_does_not_block_later_occurrence`, `exporter_registration_removes_one_exact_duplicate_occurrence`, `remove_after_snapshot_affects_only_future_records`, `exporter_removal_drops_the_exporter_outside_the_list_lock` | Recursive logging/deadlock, failure stops siblings, duplicates collapse, removal revokes retained snapshot or Drop holds list lock |
| Covered: assignment versus receipt and explicit buffer | logger.rs: `sequence_numbers_are_monotonic`, `buffer_retains_exporter_receipt_order_not_sequence_order`, `no_default_buffer_exporter_pre_registration_logs_are_dropped`, `buffer_exporter_rejects_zero_capacity`, `buffer_exporter_keeps_last_records`, `buffer_exporter_clear_empties_the_ring` | Sequence dictates concurrent receipt order, hidden default retention, zero-capacity acceptance or unbounded/wrong eviction |

Private record capture verifies production commit narration without treating its
test-only storage as a replay journal. Source review also checks that observation
registration/delivery directly uses an Observer callback rather than registering
or dispatching an observation Event: its own bookkeeping/delivery adds no recursive
Event narration. This does not suppress ordinary application operations explicitly
performed by an observer. Accepted boundaries: delivery gaps, no replay or global
order; snapshots are not globally linearizable or referentially closed; IDs grant
no control; Logger assignment sequence differs from exporter receipt order.
Known documentation deviation resolved: [example navigation](../examples/README.md)
used retired internal Event narration and historical exclusive-consumer descriptions;
it now describes the current six sources. No material evidence gap or production
defect was identified. Usage: [rule 8](consumer-guide.md#8-observe-committed-facts-and-install-explicit-logger-exporters)
and [logging_exporters](../examples/logging_exporters/src/main.rs).

## Loader plan execution and handoff

Delivery: [#207](https://github.com/dshbox/cordis-rs/issues/207).
Contract: [plan/source](v3-public-interface.md#loader-plan-and-source-schema),
[resolver/outcomes](v3-public-interface.md#loader-resolver-execution-and-outcomes),
[ADR 0036](adr/0036-module-and-crate-seams-are-semantic.md),
[sibling policy](compatibility-policy.md).
Production: [plan/execution](../crates/cordis-loader/src/plan.rs),
[resolver boundary](../crates/cordis-loader/src/resolver.rs),
[outcomes](../crates/cordis-loader/src/outcome.rs),
[handoff owner](../crates/cordis-loader/src/handoff.rs).

| Status / scenario | Discriminating evidence | Nearest rival excluded |
| --- | --- | --- |
| Covered: atomic construction, lineage and wire form | [plan.rs](../crates/cordis-loader/tests/plan.rs): `builder_accepts_only_already_admitted_same_lineage_parents_and_freezes_ids`, `failed_add_is_atomic_and_its_id_is_not_an_admitted_parent`, `validation_rejects_missing_identity_and_duplicate_axes_but_keeps_axes_orthogonal`, `source_schema_has_explicit_stable_wire_forms_and_required_plugin_config` | Failed add admits a parent, finish renumbers, foreign IDs join lineage, axes conflict or missing config silently defaults |
| Covered: strict source schema and valid execution controls | [source_schema_strict.rs](../crates/cordis-loader/tests/source_schema_strict.rs): `misspelled_disabled_is_rejected`, `misspelled_isolate_is_rejected`, `unsupported_plugin_entry_fields_are_rejected`, `fields_belonging_to_another_row_or_variant_are_rejected`, `disabled_row_does_not_execute`, `private_isolate_row_keeps_the_service_out_of_the_caller_realm`, `documented_rows_parse_and_round_trip` | A typo silently executes/defaults placement, fields cross schema objects or tagged variants, or strictness breaks valid disabled/private/round-trip rows |
| Covered: resolver adaptation before admission and normalization boundary | [resolver.rs](../crates/cordis-loader/tests/resolver.rs): `plugin_json_prepares_then_seals_synchronously`, `json_helpers_distinguish_deserialization_from_typed_preparation_without_panics`, `direct_json_helper_panics_remain_ordinary_pre_lifecycle_unwinds`; [resolver unit tests](../crates/cordis-loader/src/resolver.rs): `returned_error_normalizes_once_to_opaque_resolver_failure`, `resolver_panic_normalizes_once_without_crossing_lifecycle`, `configured_service_prepare_error_normalizes_before_any_admission`, `helper_panic_inside_resolver_normalizes_at_the_resolver_boundary` | Raw JSON enters lifecycle, helper erases typed errors/contains direct panic, resolver panic escapes, normalization repeats or adaptation failure admits residency |
| Covered: execution-local exact realms and repeated plan correlation | [realm_policy.rs](../crates/cordis-loader/tests/realm_policy.rs): `realm_policy_is_execution_local_service_exact_and_independent_of_structure`, `shared_placement_collision_fails_only_that_row_and_later_rows_continue`, `cloned_plan_reuse_preserves_entry_correlation_but_refreshes_runtime_identity` | Shared labels rendezvous across executions, structure changes placement, collision globally aborts, cloned plan loses EntryId or reuses Fiber/realm/publication identity |
| Covered: complete partial outcomes and disabled pruning | [outcome.rs](../crates/cordis-loader/tests/outcome.rs): `outcomes_are_complete_depth_first_and_pruning_names_the_disabling_plugin`, `independent_failures_do_not_prune_descendants_or_stop_later_reachable_entries`, `duplicate_resolve_keys_remain_distinct_occurrences_by_entry_order_and_fiber_handle`, `inactive_context_reports_each_reachable_plugin_and_leaves_no_runtime_residue`, `resolver_can_reenter_runtime_observation_before_lifecycle_admission` | Missing/reordered rows, ordinary failure prunes, duplicate keys collapse, inactive execution leaves residue or resolver runs under a lifecycle lock |
| Covered: rollback belongs to undelivered handoff, not ordinary row failure | [handoff.rs](../crates/cordis-loader/tests/handoff.rs): `abandonment_rolls_back_reverse_success_order_attempt_all_across_cleanup_failure_and_panic`, `abandonment_continues_after_last_input_drop_panics_between_members`, `ordinary_entry_failure_keeps_prior_success_caller_owned_and_load_remains_partial`, `delivered_outcome_drop_is_inert_and_caller_retains_fiber_handle_ownership`, `abandoned_handoff_survives_runtime_shutdown` | Abandonment strands success, first cleanup/Drop failure stops rollback, ordinary row failure rolls everything back, delivered outcome Drop disposes or origin loss cancels owner |
| Covered: no mutable topology, async resolver or key lookup API | [plan UI](../crates/cordis-loader/tests/plan_ui.rs): pass `ui-plan/pass/canonical_surface.rs`; fail `ui-plan/fail/load_plan_mutation.rs`, `load_plan_navigation.rs`, `entry_id_constructor.rs`; [resolver UI](../crates/cordis-loader/tests/resolver_ui.rs): fail `ui-resolver/fail/async_resolver.rs`, `request_representation.rs`, `resolver_failure_is_opaque.rs`; [outcome UI](../crates/cordis-loader/tests/outcome_ui.rs): fail `ui-outcome/fail/by_resolve_key.rs`, `old_split_vectors.rs` | Unapproved tree mutation/navigation, public identity construction, async adaptation, raw request/error internals or resolve-key index are usable |

Resolver unit tests exercise the production normalization boundary with private
invocation helpers; public Loader execution verifies admission and partial results.
The feature-unification consumer probes in the creation path establish actual
sibling reachability and facade exclusion. [ADR 0040](adr/0040-published-sibling-seams-are-compatibility-obligations.md)
keeps published-sibling compile obligations separate from unsupported downstream
use. Accepted boundaries: partial load, synchronous resolver, structural sequencing
without Fiber ownership, repeatable keys, correlation-only EntryId, is_ok not
universal Active, and inert post-delivery Drop. F3 is an accepted pre-1.0
Deserialize behavior change: unknown or misplaced source-schema fields now fail
before loading; valid wire forms and Serialize output remain unchanged. This
updates the supported compatibility analysis rather than claiming the former
unspecified handling already promised rejection. No unresolved defect in this path
is currently identified. Usage: [rule 9](consumer-guide.md#9-freeze-a-loader-plan-inspect-every-outcome-and-retain-delivered-handles)
and [gateway](../examples/gateway/src/main.rs).

## Timer construction, arbitration and terminal ownership

Delivery: [#208](https://github.com/dshbox/cordis-rs/issues/208).
Contract: [Timer](v3-public-interface.md#timer-facade-and-operations),
[ADR 0036](adr/0036-module-and-crate-seams-are-semantic.md),
[sibling compatibility](compatibility-policy.md).
Production: [flat facade/errors](../crates/cordis-timer/src/lib.rs),
[operations and poll kernel](../crates/cordis-timer/src/shapes.rs).

| Status / scenario | Discriminating evidence | Nearest rival excluded |
| --- | --- | --- |
| Covered: synchronous atomic refusal and validation precedence | [sleep_registration.rs](../crates/cordis-timer/tests/sleep_registration.rs): `all_public_timer_constructors_refuse_off_runtime`, `all_public_timer_constructors_refuse_without_time_driver`, `zero_interval_precedes_inactive_context`, `inactive_context_precedes_timer_environment_validation`, `unavailable_environment_precedes_deadline_range`, `out_of_range_deadline_is_a_registration_error`, `zero_sleep_is_valid_and_deadline_is_pinned_at_construction` | Born-terminal operation on refusal, driver panic, wrong precedence, rejected zero one-shot or deadline starting at first poll |
| Covered: generation admission and exact cancellation ownership | [generation_ownership.rs](../crates/cordis-timer/tests/generation_ownership.rs): `loading_active_and_permanent_root_admit_timer_operations`, `pending_failed_closing_and_disposed_refuse_all_timer_constructors`, `timer_publish_versus_close_is_refusal_or_one_delivered_cancelled_operation`, `cross_fiber_timer_use_keeps_registering_owner_restart_replaces_and_spawn_is_not_parenthood` | Gate bypass, unowned delivered timer, using Fiber becomes owner or spawn origin cancellation cascades |
| Covered: lazy caller work and panic boundary | [timeout.rs](../crates/cordis-timer/tests/timeout.rs): `construction_owns_work_without_polling_it`, `timeout_accepts_borrowing_non_send_work_and_non_send_output`, `generation_cleanup_never_owns_or_drops_timeout_work`, `cancelled_timeout_work_is_dropped_by_the_caller_poll`, `caller_work_panic_unwinds_through_timeout_poll`, `timeout_work_poll_can_reenter_generation_bookkeeping` | Eager work poll, universal Send/static bounds, cleanup moves/drops work, panic normalized as framework failure or polling under locks |
| Covered: completed/elapsed/cancelled distinctions | timeout.rs: `work_ready_before_deadline_returns_completed_output`, `deadline_already_elapsed_drops_work_without_polling_it`, `generation_cancellation_is_distinct_from_elapsed`, `standing_generation_cancellation_wins_before_work_poll`, `cancellation_wins_ready_but_uncommitted_timeout_deadline` | Elapsed polls work, cancellation becomes Elapsed or loses an uncommitted boundary |
| Covered at production-shared private kernel: work-Ready deadline recheck | [shapes.rs](../crates/cordis-timer/src/shapes.rs): `work_ready_is_rechecked_against_deadline_before_commit` | Completed commits after work polling makes the pinned deadline elapsed; removing the second deadline poll fails the discriminator |
| Covered: interval phase and terminal cancellation | [sleep_interval.rs](../crates/cordis-timer/tests/sleep_interval.rs): `interval_first_tick_is_anchored_at_construction`, `interval_late_poll_coalesces_without_burst_or_phase_shift`, `interval_cancellation_wins_uncommitted_boundary_tick`, `interval_cancellation_yields_one_error_then_ends`, `dropping_interval_emits_nothing` | Tick starts at first poll, late ticks burst/shift phase, cancellation loses boundary, multiple errors or Drop emits a tick |
| Covered: one-shot terminal contract | sleep_interval.rs: `sleep_cancellation_wins_ready_but_uncommitted_expiry`, `completed_sleep_repoll_panics`, `completed_sleep_is_not_reacted_to_by_later_generation_disposal`; timeout.rs: `completed_timeout_repoll_panics` | Cancellation loses uncommitted expiry, completed operation remains cleanup-armed, or repoll produces a second result |
| Covered: minimal bounds and no transport/alias escape | [Timer UI](../crates/cordis-timer/tests/timer_ui.rs): pass `ui-timer/pass/timeout_non_send_non_static.rs`, `named_interval_result_stream.rs`; fail `ui-timer/fail/timer_ext_is_sealed.rs`, `removed_raw_timer_transport.rs`; [facade UI](../crates/cordis-timer/tests/facade_ui.rs): pass `ui-facade59/pass/flat_root.rs`; fail `ui-facade59/fail/shapes_second_path.rs`, `timer_alias.rs`, `armed_sleep.rs` | Extra transport bounds, extensible TimerExt, raw transport, duplicate canonical path or public alias |

The work-Ready discriminator uses deterministic deadline/work doubles on the
same `poll_timeout` kernel used by production. Public integration tests cover
construction, work ownership and terminal outcomes; the private doubles arrange
one hard race, not every Tokio scheduling history. Evidence is insufficient for
stronger universal equivalence or unbounded-progress claims; those are accepted
assurance limits, not missing supported-contract discriminators. Larger paired
observer/Loader runs remain optional confidence work, as does performance
expansion. Accepted boundaries: lazy caller-owned work, ordinary work panic
unwind, pinned deadlines/phase, distinct cancellation, one-shot repoll panic and
fused post-cancellation Interval. No material evidence gap, known deviation or
production defect was found. Usage: [rule 10](consumer-guide.md#10-check-timer-registration-then-distinguish-elapsed-from-cancellation),
[worker_daemon](../examples/worker_daemon/src/main.rs), [gateway](../examples/gateway/src/main.rs).

## Aggregate disposition and assurance limits

Integration: [#209](https://github.com/dshbox/cordis-rs/issues/209) depended on the
eight path deliveries above (#201–#208), coordinated under
[spec #200](https://github.com/dshbox/cordis-rs/issues/200). Each path was delivered
against its own fixed baseline, independently reviewed on Standards and Spec,
and ran all eight local gates. The
[recommendation](api-freeze-recommendation.md) records the candidate provenance,
validation receipt and blocker dispositions.

The supported inventory is covered by the eight paths above: core default-feature
semantics, seven semantic modules, the curated core/application facade, and the
explicit Loader/Timer leaves. The naming review accounts for every canonical
specialist item, source field, operation, accessor and semantic error variant;
standard Rust trait contracts retain their ordinary meanings. Supported features
and separate published-sibling obligations remain owned by the compatibility
policy. No unsupported feature-unified __internal recipe is consumer guidance.

Current findings: F1, F2 and F4 are identified and fixed with reviewed
discriminators; F3 is an implemented, accepted pre-1.0 compatibility change; F5 is
now disclosed. No currently unresolved supported-contract production defect,
material missing discriminator or queued deliberate public break is identified.
This conclusion is conditional on this revision's validation, not the original
negative finding or CI. Evidence is insufficient
for universal equivalence, all scheduling permutations or unbounded progress;
the [concurrency record](lifecycle-concurrency-modeling.md) explicitly owns the
bounded models and public-execution evidence. Optional broader exploration does
not strengthen the contract by implication. A future actual gap or defect must
name its authority, reachable path and nearest rival and reopen candidate review.

Documentation deviations D1–D3 are resolved: architecture decision navigation
now agrees on ADRs 0028–0039 (compatibility ADR 0040 remains separate); exported
with_state appears in the canonical specialist inventory; examples describe
current observation/snapshot paths. Integration also repairs current architecture
navigation in the parity ledger and migration introduction. Historical migration
rows, audit revisions/counts and research provenance remain evidence of their
original review, not new open actions or proof for a different revision.

Accepted boundaries remain explicit in each path: independent axes, exact slots,
forward-only commit, inert Drop, explicit teardown, caller-owned Event/Timeout
work, best-effort diagnostic delivery and bounded assurance. The author excluded
[draft Wasm experiment #145](https://github.com/dshbox/cordis-rs/pull/145) from this
supported-surface review. Performance expansion measurements remain independent
follow-up; the existing benchmark baseline and review policy are not changed.
