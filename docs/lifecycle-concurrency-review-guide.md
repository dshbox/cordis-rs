# Lifecycle concurrency review guide

Status: evidence/navigation guide — not semantic authority

This guide is for reviewers changing Cordis lifecycle concurrency. It does not
restate or redefine the contract. Normative semantics remain in
[`v3-architecture.md`](v3-architecture.md),
[`v3-public-interface.md`](v3-public-interface.md), and ADRs 0028–0031.
[`lifecycle-concurrency-modeling.md`](lifecycle-concurrency-modeling.md) owns the
durable modeling record and coverage limits.

A change to `fiber/inertia.rs`, `fiber/era.rs`, Service visibility/recheck
commit ordering, generation admission/close, or public lifecycle transaction
ordering should use the tables below as a review checklist.

## Inertia / lifecycle-convergence map

| Invariant | Production transition / linearization point | Forbidden history | Reduced Loom evidence | Discriminating negative control | Real Tokio evidence | Known boundary |
| --- | --- | --- | --- | --- | --- | --- |
| **LC-01 legal states** | `InertiaSlot::{claim,kick,exit_recheck,abandon}` write only `IDLE`, `ACTIVE`, `RELEASING`; impossible observations panic | any other arbitration word is treated as healthy state | the complete `loom_inertia` transition set | invalid-state arms are defensive implementation assertions, not synthesized model states | direct Inertia unit tests plus lifecycle contracts exercise every supported transition family | reduced model does not execute production atomics |
| **LC-02 unique convergence authority** | `IDLE -> ACTIVE` CAS in `claim`/`kick`; `RELEASING -> ACTIVE` keeps the existing holder | two logical holders may settle one Fiber concurrently | `two_idle_kickers_have_one_claim_winner`, `racing_releasing_kick_preserves_single_authority` | `load_then_store_idle_claim_is_detected`, `releasing_kick_that_creates_second_holder_is_detected` | `off_runtime_recheck_claims_releasing_slot_for_the_existing_holder`; spawn-window convergence unit contract | logical authority is modeled, not Tokio task scheduling |
| **LC-03 acknowledged revision has inspection coverage** | `exit_recheck`: compute live target, read committed revision, re-inspect after a newer revision, then store `recheck_settled` | acknowledge revision `r` using target state older than `r` | `revision_acknowledgement_has_covering_target_inspection`, `two_mutations_preserve_revision_inspection_coverage` | `premature_revision_acknowledgement_without_reinspection_is_detected` | Service semantic-commit tests; `target_neutral_recheck_during_release_is_acknowledged_without_another_pass` | four-actor history is bounded; ordinary CI uses preemption 3 and scheduled assurance uses 4 |
| **LC-04 IDLE releases authority, not obligations** | Service mutation commits `recheck_committed` before any best-effort `kick`; off-runtime `IDLE` may retain `committed != settled` | executor absence consumes or hides a durable recheck | `off_runtime_commit_is_preserved_for_later_drive` | `off_runtime_commit_must_not_be_consumed_without_drive` | `off_runtime_visibility_commit_is_driven_by_later_ready` | liveness requires a later legitimate driver |
| **LC-05 racing kick preserves responsibility** | holder publishes `RELEASING`; racing kick CASes it back to `ACTIVE`, so holder's final CAS fails and it rechecks | kick erases the obligation or transfers authority to a second holder | `racing_releasing_kick_preserves_single_authority` | `releasing_kick_that_creates_second_holder_is_detected` | `off_runtime_recheck_claims_releasing_slot_for_the_existing_holder` | no claim of unbounded liveness |
| **LC-06 ready has a quiescence point** | `FiberHandle::ready` drives outstanding revisions, waits for arbitration IDLE, then rechecks `IDLE && committed == settled` | ready returns stale state when a pre-invocation mutation is already committed | `ready_return_has_a_quiescent_linearization_point` | `ready_that_treats_idle_as_quiescent_is_detected` | `ready_rechecks_after_an_old_wake_before_returning`, waiter-cancellation tests, `off_runtime_visibility_commit_is_driven_by_later_ready` | Tokio `Notify` creation/poll semantics are tested only on real Tokio |

The four-actor two-mutator model remains intentionally reduced. Local release
measurements on 2026-09-19 were approximately 0.22 s at preemption 2, 1.53 s at
preemption 3, and 7.87 s at preemption 4 after compilation. Those timings are
cost observations, not benchmarks or correctness claims. All three runs had no
permutation or duration cap; returning from `Builder::check` means the declared
finite range completed.

## Era replacement map

| Invariant | Production transition / linearization point | Forbidden history | Reduced Loom evidence | Discriminating negative control | Real Tokio evidence | Known boundary |
| --- | --- | --- | --- | --- | --- | --- |
| **ER-01 unique live-source terminal claim** | after `slot.claim`, `disposing.swap(true)` is the shared swap/dispose source claim | two swaps, or swap plus dispose, both own terminal authority | `racing_swaps_grant_one_source_claim_and_one_successor_attempt`, `disposal_race_allows_successor_only_for_a_winning_swap` | `preclaim_successor_allocation_is_detected` | `racing_replacements_claim_one_live_source_and_attempt_one_successor`, `replacement_losing_to_committed_dispose_refuses_before_successor_allocation` | lifecycle-slot uniqueness itself is inherited from LC evidence |
| **ER-02 losers allocate nothing** | losing source claim returns `Closed` before successor spawn | losing operation allocates a successor | same arbitration models | `preclaim_successor_allocation_is_detected` | replacement-vs-dispose and racing-replacement contracts | allocation internals stay real-runtime evidence |
| **ER-03 one claim, one candidate** | one committed replacement calls successor creation at most once | retry or multiple candidate allocations from one source claim | `racing_swaps_grant_one_source_claim_and_one_successor_attempt` | `successor_retry_after_one_source_claim_is_detected` | racing-replacement contract plus creation guard tests | no retry policy is a current contract, not a general recovery theorem |
| **ER-04 death before birth** | `complete_claimed_dispose().await` completes before `spawn_prepared_era_successor` | successor birth/visibility precedes old terminal barrier | `successor_attempt_observes_completed_source_disposal` | `successor_attempt_before_source_disposal_is_detected` | `old_terminal_cleanup_precedes_successor_apply` | Loom models ordering, not actual cleanup execution |
| **ER-05 closure cannot mint new authority** | preclosed source refuses; already-claimed owner uses recipe captured before it closed the source | dead source starts a new swap, or committed owner self-refuses after closing source | `preclosed_source_cannot_authorize_a_new_swap`, `committed_swap_continues_with_its_captured_recipe_after_closure` | `postclaim_closed_recheck_that_abandons_the_captured_recipe_is_detected` | `closed_source_refuses_without_respawn`, `old_terminal_cleanup_precedes_successor_apply` | recipe representation is production-only |
| **ER-06 failed candidate keeps one cleanup owner** | spawn `CreationGuard` owns pre-handoff candidate; `EraHandoffGuard` owns an undelivered successful successor | failed/undelivered successor remains resident or is cleaned twice | handoff ownership state machine | `cancelled_handoff_without_framework_cleanup_is_detected` also discriminates orphaning | successor apply failure/panic cleanup contracts and `unconsumed_success_offer_cleans_the_offered_successor` | Loom does not execute terminal cleanup |
| **ER-07 cancellation transfers no ownership back** | preclaim cancellation has no effect; postclaim detached owner continues through handoff-or-cleanup | caller cancellation orphans source/candidate/final convergence, or reclaims handed-off successor | `caller_cancellation_and_handoff_assign_one_successor_owner` | `cancelled_handoff_without_framework_cleanup_is_detected`, `cancellation_reclaiming_an_already_handed_off_successor_is_detected` | cancellation during source-claim wait, old cleanup, successor settle, failed cleanup, and final convergence | Tokio cancellation is authoritative |
| **ER-08 handoff follows final convergence** | `converge_final(...).await` completes before returning/handing off successor or `Incomplete` | success/failure becomes externally observable while affected dependents still target an intermediate era | deliberately not reduced to Loom | no synthetic Loom control: deterministic runtime barriers are the discriminating evidence | `successful_handoff_waits_for_blocked_final_dependent_recheck`, successor-failure final-convergence contract, `cancellation_during_final_dependent_recheck_finishes_no_handoff_convergence` | depends on real `ready()` / Service convergence |

The twelve Era Loom tests remain three-thread models with
`max_branches = 64`, no permutation cap, and no duration cap. The 2026-09-19
review found no new failure/cancellation combination whose semantics were both
material and better represented by another reduced Era model than by the
existing deterministic Tokio contracts.

## Lifecycle overlap / cancellation evidence matrix

This matrix is intentionally selective rather than a Cartesian product. One
arbiter serializes lifecycle ownership, so repeated pairwise tests are useful
only where they discriminate a different commit, handoff, cancellation, or
cleanup boundary.

| Boundary | Contract / forbidden early result | Evidence | Assessment |
| --- | --- | --- | --- |
| spawn / initial settle ↔ Service mutation | handoff only after live quiescence; a spawn-window convergence winner cannot steal first apply | `a_mutation_racing_the_initial_apply_is_converged_before_handoff`; direct production-path `a_convergence_winner_leaves_the_first_apply_to_the_initial_pass` | covered |
| ready ↔ Service mutation | committed drift cannot be accepted as stale quiescence | LC-06 Loom models; `ready_rechecks_after_an_old_wake_before_returning`; off-runtime Service regression | covered |
| ready ↔ era replacement | old ready follows old Fiber terminal barrier, never successor progress | `old_ready_waiter_completes_at_source_barrier_before_successor_handoff` | covered |
| restart ↔ dispose | a postcommit restart retains lifecycle authority through replacement/quiescence; terminal disposal cannot pass it | `committed_restart_serializes_terminal_dispose_behind_its_barrier` | covered by this assurance package |
| update ↔ dispose | a postcommit update retains lifecycle authority through replacement/quiescence; terminal disposal cannot pass it | `committed_update_serializes_terminal_dispose_behind_its_barrier` | covered by this assurance package |
| restart ↔ era swap | era source claim waits behind an in-flight restart; cancellation while waiting remains preclaim/no-effect | `cancellation_while_waiting_for_source_claim_is_no_effect` | covered |
| update ↔ era swap | era cannot claim the source while a committed update owns the lifecycle slot; after update quiescence it may replace the updated source | `committed_update_serializes_era_swap_until_update_quiescence` | covered by this assurance package |
| era swap ↔ dispose | one source terminal authority; loser allocates nothing | ER-01/02 Loom and `replacement_losing_to_committed_dispose_refuses_before_successor_allocation`; later dispose waits only old Fiber | covered |
| era swap ↔ era swap | exactly one live-source claim and one candidate | ER-01/03 Loom and racing-replacements Tokio contract | covered |
| concurrent dispose ↔ dispose | all terminal callers coalesce on the winning full barrier | `concurrent_disposals_coalesce_through_cleanup_disposed_and_unlink_after_winner_cancellation` | covered |
| precommit update control ↔ dispose | dispose may invalidate admission while control awaits; old generation/config remain authoritative | `close_during_awaited_control_reports_admission_lost` | covered |
| caller cancellation before lifecycle commit | no state/config/gate/claim/allocation effect | spawn pre-admission refusal/cancellation contracts; update precommit cancellation; era source-claim-wait cancellation | covered at representative commits |
| caller cancellation after lifecycle commit | framework owner completes cleanup/quiescence/handoff independently | spawn, restart, update, disposal, era cancellation contracts | covered at each deep operation family |
| generation close ↔ retained-resource registration | either refusal with no occurrence, or one owned commit later cleaned exactly once | `every_core_family_publish_versus_close_has_only_refusal_or_owned_commit`, effect registration race tests | covered |
| generation close ↔ manual retained-resource control | one exact cleanup claim; no duplicate cleanup | effect dispose/disarm-vs-drain tests, Service publication stale/current control tests | covered |
| Service same-occurrence payload `set` ↔ ready | payload mutation is target-neutral by contract, so no lifecycle drift overlap exists | Service target-neutral tests / ADR 0031 | semantically irrelevant, not a missing race test |

## Reviewer trigger

When an atomic/state transition changes in `inertia.rs` or `era.rs`:

1. Identify which LC/ER row changes before editing a model.
2. Recheck the named production linearization point against ADR 0029/0030/0031.
3. Run the row's positive reduced model and its negative control; a negative
   control that no longer fails has lost discriminating power.
4. Run the named Tokio contract because Loom does not substitute for Tokio
   notification, task cancellation, cleanup, or Service reconstruction.
5. For Inertia changes affecting revision/inspection ordering, also run the
   four-actor model at the scheduled preemption bound.
6. If production/model mapping has materially changed, update
   `lifecycle-concurrency-modeling.md` in the same change. Do not genericize
   production synchronization merely to make a model share code unless drift has
   become a demonstrated maintenance defect.

Passing these bounded checks is evidence within their declared ranges. It is not
formal verification, weak-memory proof, or an unbounded liveness claim.
