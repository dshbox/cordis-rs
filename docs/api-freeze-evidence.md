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
