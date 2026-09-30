# API freeze candidate record

This non-normative record supplies the evidence for, and records, the author's
API freeze candidate decision under
[spec #200](https://github.com/dshbox/cordis-rs/issues/200) and
[#222](https://github.com/dshbox/cordis-rs/issues/222). It does not alter
compatibility promises or authorize 1.0.
The [public interface](v3-public-interface.md), [architecture](v3-architecture.md),
[glossary](../CONTEXT.md), accepted ADRs, [parity ledger](v3-upstream-parity-ledger.md)
and [compatibility policy](compatibility-policy.md) retain authority.

## Declaration

On 2026-09-30 the author declared the API freeze candidate in
[#222](https://github.com/dshbox/cordis-rs/issues/222):

- **Candidate commit:** `main` commit
  `64aa6deddf8678100648927e9311c0a7618bf53f`, the merge of
  [#244](https://github.com/dshbox/cordis-rs/pull/244).
- **Versions:** semantic `cordis-core`, `cordis-loader` and `cordis-timer`
  0.6.0 and facade `cordis-rs` 0.11.0, released by
  [#239](https://github.com/dshbox/cordis-rs/pull/239) and published. The
  candidate's source, normative documents and fixtures equal the release tags
  (`cordis-core-v0.6.0`, `cordis-loader-v0.6.0`, `cordis-timer-v0.6.0` and
  `v0.11.0`, all at `010df45`) except the test-only
  `crates/cordis-core/tests/idle_origin_runtime.rs`
  ([#242](https://github.com/dshbox/cordis-rs/pull/242) and #244).
- **Validation:** [main CI run 36695938925](https://github.com/dshbox/cordis-rs/actions/runs/36695938925)
  on that exact commit passed all 10 jobs of the CI workflow: Formatting,
  Clippy, docs, vocabulary; Latest stable compatibility; Dependency audit and
  policy; Loom concurrency models; MSRV (Rust 1.88); Package release crates;
  Test canonical Rust on ubuntu-latest, macos-latest and windows-latest; and
  Run examples.
- **Candidate 1.0 surface:** the normative
  [public-interface inventory](v3-public-interface.md) at that commit,
  including its [openness and auto-trait declarations](v3-public-interface.md#openness-and-auto-traits),
  is the candidate 1.0 surface.

The declaration change edits only non-normative records: this record, the
[conformance evidence](api-freeze-evidence.md), the
[naming review](api-naming-review.md), the ROADMAP checkbox and the README
status lines. It leaves the public interface, architecture, ADRs,
compatibility policy, glossary and crate sources as they are at the candidate.
The weekly correctness-assurance workflow is a ROADMAP confidence goal, not a
required job. Its latest run,
[run 36413948134](https://github.com/dshbox/cordis-rs/actions/runs/36413948134)
(2026-09-28 on `ed07d31`), predates and does not cover the candidate; a
`workflow_dispatch` on the candidate is optional. The pending test-only
release PR [#243](https://github.com/dshbox/cordis-rs/pull/243)
(0.6.1 / 0.11.1) is not part of the candidate and stays unmerged until after
this declaration. Any release cut from a later commit is a post-freeze release
governed by the compatibility policy; the stabilization release is a separate,
later ROADMAP gate, and whether a given release counts as it is the author's
decision.

## Candidate and exact revision

**The API freeze candidate is `main` commit
`64aa6deddf8678100648927e9311c0a7618bf53f` (semantic 0.6.0 / facade 0.11.0),
validated by main CI run 36695938925 (10/10 jobs).** Later correctness
review identified defects omitted by the original investigation. F1 (stale ready
observation), F2 (idle-origin committed completion) and F4 (deadline scheduling)
are now fixed and reviewed; F3 is an accepted and implemented strict Loader
Deserialize change. F5's apply-placement disclosure is included here. F6 is an
accepted, incorporated narrowing of user destructor panics to the best-effort
ADR 0041 rule. The later pre-freeze surface scan
[#224](https://github.com/dshbox/cordis-rs/issues/224) found two
anticipated-growth breaks (A1 exhaustive vocabularies, A2 literal-only Loader
source rows), four contract gaps (B1 committed completion that depended on
caller polling, B2 missing compatible-evolution rules, B3 undefined admission
order, B4 `ObservationRouting` listed as `Copy`) and three defects (C1 the
`wait_state` deadline tie, C2 a forgeable `EffectFailure`, C3 the `FiberHandle`
rustdoc). All nine were resolved by
[#225](https://github.com/dshbox/cordis-rs/issues/225) (PRs #233–#237, #240
and #241) and released as semantic 0.6.0 / facade 0.11.0 (#239). No currently
unresolved supported-contract defect or queued deliberate public break is
identified.
All ten consumer rules and the updated naming inventory are delivered. The
author declared the candidate in #222; 1.0 remains a later decision.

Production source baseline is the candidate `64aa6de`, fetched on 2026-09-30.
PR [#210](https://github.com/dshbox/cordis-rs/pull/210) prepared this record at
`4824196` (merged as `30c96e3`), after
[#214](https://github.com/dshbox/cordis-rs/pull/214),
[#216](https://github.com/dshbox/cordis-rs/pull/216),
[#217](https://github.com/dshbox/cordis-rs/pull/217),
[#218](https://github.com/dshbox/cordis-rs/pull/218) and release
[#215](https://github.com/dshbox/cordis-rs/pull/215); its preparation added
documentation only, including Plugin apply rustdoc and the with_state inventory
entry for an already-exported item. Since then the candidate adds
[#221](https://github.com/dshbox/cordis-rs/pull/221) (ADR 0041), the 0.4.1/0.9.1
and 0.5.0/0.10.0 releases (#219, #223), the #225 batch (#233–#237, #240,
#241), the 0.6.0/0.11.0 release #239 and the test-only #242/#244. Affected
conformance, naming, schema, openness and scheduling claims are rechecked
against it. Validation of an earlier baseline or revision does not validate
this candidate.

The candidate is `64aa6de` with main CI run 36695938925. Because it precedes
this declaration, the record names it directly. The declaration's own merge
commit changes only this record, the conformance evidence, the naming review,
the consumer guide's link to this record, the ROADMAP checkbox and the README
status lines. Its diff against `64aa6de` is
empty under `crates/`, the public interface, the architecture, the ADRs, the
compatibility policy and the glossary, as recorded in
[PR #245](https://github.com/dshbox/cordis-rs/pull/245), so the declared
surface is unchanged. The candidate remains `64aa6de`: a later commit,
including this declaration's merge, is not itself the candidate and does not
inherit its CI validation by branch name. The declared surface stays the
normative inventory at `64aa6de` while later changes are compatible under the
[compatibility policy](compatibility-policy.md) and ADR 0042. Naming any later
commit as a candidate requires rechecking the affected claims and that commit's
own main CI run. A deliberate break updates its
authority and restarts the ROADMAP stabilization requirement. The stabilization
release ([#243](https://github.com/dshbox/cordis-rs/pull/243) or a successor)
is a separate, later ROADMAP gate. This is not a 1.0 readiness claim.

## Deliverables and coverage

- [Consumer guide](consumer-guide.md): exactly ten practical rules with owning
  authority, boundaries, runnable source and commands; reachable from both
  [English README](../README.md) and [Chinese README](../README.zh-CN.md).
- [Conformance evidence](api-freeze-evidence.md): named discriminating tests and
  nearest rivals, source paths, private-seam limits, known documentation deviations
  and accepted boundaries for eight complete consumer paths.
- [Public naming review](api-naming-review.md): Retain / Clarify / Propose rename
  conclusions for supported names, members, variants, fields and accessors;
  no new rename proposal or public alias.
- Current architecture navigation, with_state inventory and example descriptions
  reconciled. Historical failure, concurrency and migration records retain their
  original baselines and bounded coverage rather than become a live defect queue.

Scope includes the default-feature cordis-core contract, supported cordis
application re-exports and explicit cordis-loader/cordis-timer leaves. No supported
opt-in downstream core feature exists. Cargo unification can expose direct-core
__internal through sibling dependencies; it remains unsupported downstream.
[ADR 0040](adr/0040-published-sibling-seams-are-compatibility-obligations.md) separately
requires already-published siblings to compile across admitted core versions.
[ADR 0042](adr/0042-public-evolution-declares-openness-and-auto-traits.md) and the
policy's [compatible evolution](compatibility-policy.md#compatible-evolution)
declare every public enum Open or Closed, freeze error-variant fields, and
promise the listed auto traits and operation-future `Send` conditions as part
of the candidate surface.
The pre-v3 `legacy/0.6` line and its companion crates, and the author-excluded
draft Wasm experiment, are outside this candidate.

## Blocker register

This register uses evidence and authority, not merely open-issue count. The eight
original path tickets remain closed; #209's original completion receipt is history.
New material findings reopen candidate review even when those tickets stay closed.
The candidate includes the follow-up PRs below and the #225 pre-freeze batch.

| Item / disposition | Authority and evidence | Owner / dependency / resolution |
| --- | --- | --- |
| Supported-contract defects: identified → fixed → reviewed (F1, F2, F4) | [#214](https://github.com/dshbox/cordis-rs/pull/214): stale ready/restart race; [#216](https://github.com/dshbox/cordis-rs/pull/216): ADR 0029 idle-current-thread completion; [#218](https://github.com/dshbox/cordis-rs/pull/218): shared deadlines, contained wakes and spawn-refusal error. Added named regressions are in the [evidence](api-freeze-evidence.md#findings-since-the-original-review) | Maintainer; the three defect-fix PRs are merged into the source baseline, affected claims rechecked here; any new proven defect blocks selection until fixed and reviewed |
| Strict Loader schema: accepted pre-1.0 change → implemented and reviewed (F3) | [#217](https://github.com/dshbox/cordis-rs/pull/217), interface, migration note and [Loader changelog](../crates/cordis-loader/CHANGELOG.md); unknown source-schema fields now fail Deserialize, valid wire forms and Serialize unchanged | Author/maintainer; already accepted and merged, with public parsing/execution controls; not an unresolved break or a retrospective promise about unspecified former behavior |
| DeadlineUnavailable inventory: additive variant → reconciled | #218 adds `WaitStateError::DeadlineUnavailable` on a non-exhaustive enum; interface/error inventory, naming review and guide now account for it | Maintainer; #218 merged first, no alias or rename migration; consumers retain wildcard matching |
| Apply-placement disclosure: identified → documented (F5) | Plugin::apply rustdoc, interface and guide rules 1 and 5 state the consumer contract; ADR 0029 completion executor posture records current placement and sizing | Maintainer/consumer; disclosure resolved in this follow-up. Apply polls must not block; offload synchronous blocking sections and await them asynchronously. Apply has no origin-runtime affinity and may run on a Cordis-owned runtime; which runtime and its worker count are current ADR 0029 posture, not frozen. Earlier captured resources depend on their original driver; the caller's paused clock does not control apply polled elsewhere. No progress while consumers block Cordis-owned workers is promised |
| User destructor panic contract: identified → decided → documented before freeze (F6) | [#220](https://github.com/dshbox/cordis-rs/issues/220) and [#221](https://github.com/dshbox/cordis-rs/pull/221); [ADR 0041](adr/0041-user-destructor-panics-are-best-effort.md), added after the #210 baseline, replaces per-site destructor clauses with one [best-effort rule](v3-public-interface.md#user-destructor-panics); see the [evidence](api-freeze-evidence.md#findings-since-the-original-review) | Author/maintainer; resolved before the freeze decision as a documentation-only contract narrowing, with runtime behavior and containment tests unchanged. It is marked breaking for release notes and was released with its [migration](../MIGRATION.md) section in semantic 0.5.0 / facade 0.10.0 ([#223](https://github.com/dshbox/cordis-rs/pull/223)). Stronger destructor guarantees may be added later compatibly; a new destructor finding is a best-effort robustness fix and reopens no freeze criterion unless it also breaks an unchanged contract such as ADR 0029 lock discipline or inert handle Drop |
| Pre-freeze surface scan [#224](https://github.com/dshbox/cordis-rs/issues/224): identified → decided ([#225](https://github.com/dshbox/cordis-rs/issues/225)) → fixed/documented → released | A1 [#235](https://github.com/dshbox/cordis-rs/pull/235), A2 [#233](https://github.com/dshbox/cordis-rs/pull/233), C1 [#234](https://github.com/dshbox/cordis-rs/pull/234), C2 [#236](https://github.com/dshbox/cordis-rs/pull/236), B1 [#237](https://github.com/dshbox/cordis-rs/pull/237), B3/B4/C3 [#240](https://github.com/dshbox/cordis-rs/pull/240), B2 [#241](https://github.com/dshbox/cordis-rs/pull/241) with ADR 0042; released in [#239](https://github.com/dshbox/cordis-rs/pull/239) (semantic 0.6.0 / facade 0.11.0) with its [migration](../MIGRATION.md) section; see the [evidence](api-freeze-evidence.md#findings-since-the-original-review) | Author/maintainer; resolved before the freeze. #224's unverified U1 (held Loader load) is now covered by the Loader held-load tests; U2 is subsumed by B3's unspecified order among waiting intents; D1–D7 need no action |
| Material missing discriminators: original omissions remedied; none currently open | Added stale-transient, idle-origin, strict-schema and deadline/waker discriminators; the #225 batch added held-caller dispose/removal tests (`held_caller_completion.rs`, the `idle_origin_runtime.rs` held-caller tests and private `caller_driven_*` tests), held spawn/load narrowing tests, the `wait_state` tie (`wait_state_publication_tie.rs`), the `cleanup_result_outcome_*` UI fail fixtures (C2), in-crate `vocabulary_inventory` tests in core and Loader (A1), `constructors_equal_the_deserialized_default_rows` (A2), the `ui-auto-traits` pass fixtures in core, Loader and Timer (B2) and `update_admission_follows_control_completion_not_call_order` (B3); private scheduling/refusal seams explicitly bounded in the evidence | Maintainer; local gates and exact-candidate CI validate the revised record; an actual new gap needs its reachable rival and a blocking ticket |
| Queued deliberate public breaks / accepted new renames: none currently identified | F3, the F4 additive variant, the F6 destructor narrowing and the 0.6/0.11 batch (A1 opened vocabularies, A2 extensible Loader rows, the B1 creation-progress narrowing and the B3 admission-order narrowing) are already incorporated and released; current member-level naming review retains accepted ADRs and proposes no new rename | Author; a future approved rename gets its own ticket, migration impact and true blocking edges |
| D1–D3 documentation drift: resolved | Decision index/count and navigation; existing with_state export; six current runnable example sources | #201, #205, #206 and #209 |
| Exact candidate CI receipt: resolved | [main CI run 36695938925](https://github.com/dshbox/cordis-rs/actions/runs/36695938925) on `64aa6de`: 10/10 jobs (formatting/clippy/docs/vocabulary, latest stable, audit/policy, Loom, MSRV 1.88, package, canonical tests on ubuntu/macOS/windows, examples) | CI/maintainer; the weekly correctness-assurance lane is a confidence goal, not a required job |
| Draft Wasm #145: excluded, not a queued break | [Experiment/wasm components](https://github.com/dshbox/cordis-rs/pull/145); author confirmed independent scope during planning | Author/experiment owner; no dependency on this candidate, future workspace/MSRV decisions remain separate |
| Broader state-space/performance expansion: independent follow-up | [Bounded concurrency evidence](lifecycle-concurrency-modeling.md), [performance baseline/policy](performance-benchmarking.md), ROADMAP confidence goals | Maintainer; outside this preparation, no newly invented semver gate |

Tracker inspection on 2026-09-30 found #200, #220, #224, #225 and its child
tickets closed, #222 open, [release #239](https://github.com/dshbox/cordis-rs/pull/239)
merged and published as semantic 0.6.0 / facade 0.11.0, the release-plz PR
[#243](https://github.com/dshbox/cordis-rs/pull/243) (0.6.1 / 0.11.1,
test-only) open, and the excluded Wasm draft #145 independent. Consumers
upgrade direct core/timer/loader requirements together to `0.6` and the facade
to `0.11` ([migration](../MIGRATION.md)). Merging a release preparation does
not replace the exact candidate's review and CI receipt.
Merged tracker state alone does not prove correctness or absence of further defects.
The updated source-to-contract review and named regressions support the current
finding; the original "none identified" statement is superseded, not preserved as
a claim for the candidate. No known unresolved supported-contract item is deferred
to make the candidate appear ready.

## Review and validation record

Each delivery ([#201](https://github.com/dshbox/cordis-rs/issues/201)–[#209](https://github.com/dshbox/cordis-rs/issues/209))
and each earlier revision of this preparation was reviewed on separate Standards and Spec
axes and validated by the eight local gates: toolchain, fmt, clippy, vocabulary,
tests, docs, examples and floating-latest compatibility. Canonical UI snapshots
use the pinned Rust toolchain and rust-src; floating latest checks compatibility
only. Local gates do not duplicate the explicit MSRV, audit/policy, Loom or
release-package CI jobs; none is silently replaced by a local green test.

Commit-level receipts (fixed points, result commits, review finding counts, gate
run identifiers and CI runs) live in the delivery tickets and in
[PR #210](https://github.com/dshbox/cordis-rs/pull/210)'s description and
comments, not in this document. The #225 batch PRs (#233–#237, #240, #241) were
each reviewed on Standards and Spec and validated by the local gates and main
CI; their receipts live in those PRs and in #225. The candidate receipt is
recorded in [PR #245](https://github.com/dshbox/cordis-rs/pull/245); the
[#222](https://github.com/dshbox/cordis-rs/issues/222) closing comment links it.
The docs-only declaration revision ran the toolchain, fmt, vocabulary and docs
gates; its candidate is validated by main CI run 36695938925.

## Residual limits and later 1.0 work

Tests and selected bounded models do not establish universal equivalence,
every scheduling permutation or unbounded progress. Best-effort observations
are not an audit journal, snapshots are not globally linearizable, exact realms
have no fallback, Drop is inert, teardown is explicit, caller Event/Timeout work
stays caller-owned and external driver lifetime/process termination limits remain.
Apply must keep polls non-blocking, cannot assume origin-runtime affinity and may
run on a Cordis-owned runtime; a blocked poll can stall unrelated lifecycle work.
These are disclosed authority boundaries rather than unfinished executor-isolation
promises; current placement and sizing remain ADR 0029 posture, not frozen.
A committed creation does not advance while its spawn or load future is held
unpolled, and work waiting on the new Fiber waits with it; dropping that future
hands the creation to framework completion, which fully disposes and unlinks
the undelivered Fiber. Among lifecycle intents already waiting for the arbiter,
admission order is unspecified. At the `wait_state` deadline tie, a requested
state published before the deadline is always observed; one published after the
deadline but before the waiter resumes may be reported either way.

The author declared `64aa6de` the API freeze candidate in #222 and ticked the
[ROADMAP](../ROADMAP.md#required-before-10) item "Declare an API freeze
candidate". ROADMAP still requires one stabilization release without a planned
break. A deliberate break updates authority and restarts that stabilization
requirement. Before 1.0, finish stable-promise discovery wording, the final
pre-1.0-to-1.0 migration note (or explicit no-change statement), and CI/package
verification for the actual release commit. Other ROADMAP checkboxes and the
pre-1.0 status wording remain unchanged. This effort neither performs those
releases nor expands performance measurements.
