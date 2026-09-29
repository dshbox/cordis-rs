# API freeze candidate recommendation

This non-normative recommendation supplies evidence for the author's decision
under [spec #200](https://github.com/dshbox/cordis-rs/issues/200). It does not
declare freeze, alter compatibility promises, merge a PR or authorize 1.0.
The [public interface](v3-public-interface.md), [architecture](v3-architecture.md),
[glossary](../CONTEXT.md), accepted ADRs, [parity ledger](v3-upstream-parity-ledger.md)
and [compatibility policy](compatibility-policy.md) retain authority.

## Recommendation and exact revision

**Recommend the assembled supported v3 surface as an API freeze candidate,
conditional on the updated exact revision's validation receipt.** Later correctness
review identified defects omitted by the original investigation. F1 (stale ready
observation), F2 (idle-origin committed completion) and F4 (deadline scheduling)
are now fixed and reviewed; F3 is an accepted and implemented strict Loader
Deserialize change. F5's apply-placement disclosure is included here. No currently
unresolved supported-contract defect or queued deliberate public break is identified.
All ten consumer rules and the updated naming inventory are delivered. Freeze
remains the author's explicit subsequent decision.

Production source baseline is
`680b3b058659a6318e51ec174537a424118f2927`, fetched on 2026-09-29 after
[#214](https://github.com/dshbox/cordis-rs/pull/214),
[#216](https://github.com/dshbox/cordis-rs/pull/216),
[#217](https://github.com/dshbox/cordis-rs/pull/217) and
[#218](https://github.com/dshbox/cordis-rs/pull/218) merged. PR #210 is rebased onto
that revision; affected conformance, naming, schema and scheduling claims are
rechecked against it. The original `ed07d31` investigation and `060c33b` candidate
receipt below remain historical. Their green checks do not validate this candidate.
The preparation diff adds documentation, including Plugin apply rustdoc; it does
not change production behavior, dependencies, toolchain, features or golden stderr.
The with_state correction still records an already-exported item.

The updated exact candidate is the resulting commit recorded with its CI run and
review/gate summaries in [PR #210's validation receipt](https://github.com/dshbox/cordis-rs/pull/210).
This avoids an impossible self-hash inside a versioned file. A later commit is a
different candidate: recheck affected claims and CI instead of inheriting a green
result by branch name. Until the updated receipt confirms final-head CI, the
recommendation is conditional. This is not a 1.0 readiness claim or a freeze
announcement.

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
Legacy 0.6 and the author-excluded draft Wasm experiment are outside this candidate.

## Blocker register

This register uses evidence and authority, not merely open-issue count. The eight
original path tickets remain closed; #209's original completion receipt is history.
New material findings reopen candidate review even when those tickets stay closed.
The current source baseline includes the four merged follow-up PRs below.

| Item / disposition | Authority and evidence | Owner / dependency / resolution |
| --- | --- | --- |
| Supported-contract defects: identified → fixed → reviewed (F1, F2, F4) | [#214](https://github.com/dshbox/cordis-rs/pull/214): stale ready/restart race; [#216](https://github.com/dshbox/cordis-rs/pull/216): ADR 0029 idle-current-thread completion; [#218](https://github.com/dshbox/cordis-rs/pull/218): shared deadlines, contained wakes and spawn-refusal error. Added named regressions are in the [evidence](api-freeze-evidence.md#findings-since-the-original-review) | Maintainer; the three defect-fix PRs are merged into the source baseline, affected claims rechecked here; any new proven defect blocks selection until fixed and reviewed |
| Strict Loader schema: accepted pre-1.0 change → implemented and reviewed (F3) | [#217](https://github.com/dshbox/cordis-rs/pull/217), interface, migration note and [Loader changelog](../crates/cordis-loader/CHANGELOG.md); unknown source-schema fields now fail Deserialize, valid wire forms and Serialize unchanged | Author/maintainer; already accepted and merged, with public parsing/execution controls; not an unresolved break or a retrospective promise about unspecified former behavior |
| DeadlineUnavailable inventory: additive variant → reconciled | #218 adds `WaitStateError::DeadlineUnavailable` on a non-exhaustive enum; interface/error inventory, naming review and guide now account for it | Maintainer; #218 merged first, no alias or rename migration; consumers retain wildcard matching |
| Apply-placement disclosure: identified → documented (F5) | ADR 0029 completion executor posture, Plugin::apply rustdoc, interface and guide rule 1; initial ordinary settle is caller-driven, later framework passes use shared completion workers | Maintainer/consumer; disclosure resolved in this follow-up. Apply polls must not block; offload synchronous blocking sections and await them asynchronously. Earlier captured resources depend on their original driver; the caller's paused clock does not control later apply. No origin-runtime affinity or progress while consumers block both current completion workers is promised |
| Material missing discriminators: original omissions remedied; none currently open | Added stale-transient, idle-origin, strict-schema and deadline/waker discriminators; private scheduling/refusal seams explicitly bounded in the evidence | Maintainer; updated local gates and exact-head CI must validate the revised record; an actual new gap needs its reachable rival and a blocking ticket |
| Queued deliberate public breaks / accepted new renames: none currently identified | F3 and the F4 additive variant are already incorporated; current member-level naming review retains accepted ADRs and proposes no new rename | Author; a future approved rename gets its own ticket, migration impact and true blocking edges |
| D1–D3 documentation drift: resolved | Decision index/count and navigation; existing with_state export; six current runnable example sources | #201, #205, #206 and #209; retained after rebase |
| Updated exact final-head CI receipt: verification condition | PR #210 receipt, updated eight local gates and CI workflow; historical candidate CI is not final-head CI | CI/maintainer; resolve only when required jobs pass for the recorded new commit/head/run |
| Draft Wasm #145: excluded, not a queued break | [Experiment/wasm components](https://github.com/dshbox/cordis-rs/pull/145); author confirmed independent scope during planning | Author/experiment owner; no dependency on this candidate, future workspace/MSRV decisions remain separate |
| Broader state-space/performance expansion: independent follow-up | [Bounded concurrency evidence](lifecycle-concurrency-modeling.md), [performance baseline/policy](performance-benchmarking.md), ROADMAP confidence goals | Maintainer; outside this preparation, no newly invented semver gate |

Tracker inspection on 2026-09-29 found parent spec #200 open, this PR,
[release PR #215](https://github.com/dshbox/cordis-rs/pull/215) and the excluded
Wasm draft. The pending release PR targets semantic packages `0.4.0` and the
independently versioned facade `0.9.0`. It preserves F3's breaking Deserialize
note and F4's additive variant note. The new semantic line records the accepted
input behavior change; the facade line coordinates the identity of its public
core re-exports with `cordis-core 0.4`. Consumers must upgrade direct core/timer/loader
requirements together to `0.4` and the facade to `0.9`, as documented in that
release PR. This pending release plan does not replace the exact candidate's
review and CI receipt.
Merged tracker state alone does not prove correctness or absence of further defects.
The updated source-to-contract review and named regressions support the current
finding; the original "none identified" statement is superseded, not preserved as
a claim for current main. No known unresolved supported-contract item is deferred
to make the candidate appear ready.

## Review and local validation chain

The historical 2026-09-28 delivery chain below pinned each starting commit, reviewed
the complete diff on separate
Standards and Spec axes, and completed all eight local gates. The two numbers in
review columns are unresolved findings, not a merger of the axes. Initial findings
were resolved before commit. Full fixed baselines and review/gate summaries are in the linked
ticket completion records.

| Delivery | Fixed point | Result commit | Standards / Spec | Local gates and log directory under target/gates |
| --- | --- | --- | --- | --- |
| [#201](https://github.com/dshbox/cordis-rs/issues/201) | `ed07d31` | [`dd53012`](https://github.com/dshbox/cordis-rs/commit/dd530121af77754884296b75c76c69f0e3bc24b6) | 0 / 0 | 8/8 PASS; `freeze-201-20260928-01` |
| [#202](https://github.com/dshbox/cordis-rs/issues/202) | `dd53012` | [`b02b1cf`](https://github.com/dshbox/cordis-rs/commit/b02b1cf167b741352a5401b681318e05b0ec6de6) | 0 / 0 | 8/8 PASS; `freeze-202-20260928-01` |
| [#203](https://github.com/dshbox/cordis-rs/issues/203) | `b02b1cf` | [`0363f4b`](https://github.com/dshbox/cordis-rs/commit/0363f4b8b14a8a8c23d89833a5648d94e57d4389) | 0 / 0 | 8/8 PASS; `freeze-203-20260928-01` |
| [#204](https://github.com/dshbox/cordis-rs/issues/204) | `0363f4b` | [`0f72395`](https://github.com/dshbox/cordis-rs/commit/0f723954db2251e364aab3dc44f6c850698b7bcc) | 0 / 0 | 8/8 PASS; `freeze-204-20260928-01` |
| [#205](https://github.com/dshbox/cordis-rs/issues/205) | `0f72395` | [`d88129d`](https://github.com/dshbox/cordis-rs/commit/d88129d5cc5794a98978cebc96f6976e6c0789fe) | 0 / 0 | 8/8 PASS; `freeze-205-20260928-01` |
| [#206](https://github.com/dshbox/cordis-rs/issues/206) | `d88129d` | [`51664d2`](https://github.com/dshbox/cordis-rs/commit/51664d26cc6dd8d7a31dd5a468d4828f1be6bfef) | 0 / 0 | 8/8 PASS; `freeze-206-20260928-01` |
| [#207](https://github.com/dshbox/cordis-rs/issues/207) | `51664d2` | [`7405989`](https://github.com/dshbox/cordis-rs/commit/740598906c7e730a073da4dd833dc1a647a693fc) | 0 / 0 | 8/8 PASS; `freeze-207-20260928-01` |
| [#208](https://github.com/dshbox/cordis-rs/issues/208) | `7405989` | [`753f534`](https://github.com/dshbox/cordis-rs/commit/753f534191ad8c4f4c029e02e6dfb68bd3015879) | 0 / 0 | 8/8 PASS; `freeze-208-20260928-01` |
| [#209](https://github.com/dshbox/cordis-rs/issues/209) | `753f534` | `060c33b` in original completion receipt | Separate original integration reports in receipt | 8/8 PASS; `freeze-209-20260928-01` |

These pre-rebase commit IDs and receipts preserve the original review history. They
are not the new rebased commits or evidence of a gate run on the updated candidate.
The follow-up reviews the complete current preparation diff against fixed source
baseline `680b3b0` on Standards and Spec axes, then runs eight updated local gates
under `freeze-210-review-20260929-01`. ADR alignment and blocking-section/driver
clarifications then use follow-up run `freeze-210-f5-20260929-01`. Results and
exact-head CI for the final updated commit belong in the PR #210 receipt;
the intermediate `b3b8210` checks do not validate that later revision.

Each run covers toolchain, fmt, clippy, vocabulary, tests, docs, examples and
floating-latest compatibility. Cached dependencies were used with
CARGO_NET_OFFLINE=true; canonical UI snapshots used the pinned Rust 1.98.1 and
rust-src. Latest checks compatibility only. Planning's initial DNS failure was
resolved by rerunning only its failed test gate offline; it was not a code failure.
No implementation ticket needed that retry. Gate logs are local artifacts; ticket
receipts retain the summaries even if routine target cleanup removes older logs.

The original source baseline's [CI run](https://github.com/dshbox/cordis-rs/actions/runs/36353799979)
passed MSRV 1.88, dependency audit/policy, canonical tests on Linux/macOS/Windows,
Loom models, latest stable, formatting/Clippy/docs/vocabulary, all six examples
and release-package lanes. The original candidate
[CI run](https://github.com/dshbox/cordis-rs/actions/runs/36367061979) passed for
`060c33b`, as recorded in #209. Updated-candidate CI must be read separately from
the PR #210 receipt. Local gates do not duplicate the explicit MSRV, audit/policy, Loom
or release-package jobs; none is silently replaced by a local green test.

## Residual limits and later 1.0 work

Tests and selected bounded models do not establish universal equivalence,
every scheduling permutation or unbounded progress. Best-effort observations
are not an audit journal, snapshots are not globally linearizable, exact realms
have no fallback, Drop is inert, teardown is explicit, caller Event/Timeout work
stays caller-owned and external driver lifetime/process termination limits remain.
Apply must keep polls non-blocking and cannot assume origin-runtime affinity;
blocking the shared completion workers can stall unrelated lifecycle work. These
are disclosed authority boundaries rather than unfinished executor-isolation promises.

[ROADMAP](../ROADMAP.md#required-before-10) still requires the author to select
and declare the candidate; then complete a stabilization release without a planned
break. A deliberate break updates authority and restarts that stabilization
requirement. Before 1.0, finish stable-promise discovery wording, the final
pre-1.0-to-1.0 migration note (or explicit no-change statement), and CI/package
verification for the actual release commit. Current pre-1.0 status wording and
ROADMAP checkboxes remain unchanged. This effort neither performs those releases
nor expands performance measurements.
