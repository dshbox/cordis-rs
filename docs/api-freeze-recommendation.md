# API freeze candidate recommendation

This non-normative recommendation supplies evidence for the author's decision
under [spec #200](https://github.com/dshbox/cordis-rs/issues/200). It does not
declare freeze, alter compatibility promises, merge a PR or authorize 1.0.
The [public interface](v3-public-interface.md), [architecture](v3-architecture.md),
[glossary](../CONTEXT.md), accepted ADRs, [parity ledger](v3-upstream-parity-ledger.md)
and [compatibility policy](compatibility-policy.md) retain authority.

## Recommendation and exact revision

**Recommend the assembled supported v3 surface as an API freeze candidate,
conditional on the exact final revision's validation receipt.** The investigation
found no unresolved supported-contract production defect, material evidence gap
or queued deliberate public break. All ten consumer rules and the complete naming
review are delivered. Freeze remains the author's explicit subsequent decision.

Production source baseline is
`ed07d3149a5711541eaabd34633122e6eb3c52bd`, fetched and repeatedly checked against
origin/main on 2026-09-28. The eight reviewed consumer-path deliveries end at
`753f534191ad8c4f4c029e02e6dfb68bd3015879`; that is the fixed point for the final
integration diff. All changes in this effort are Markdown: production Rust,
dependencies, toolchain, feature definitions, golden stderr and gate inputs are
unchanged. The with_state inventory correction records an already-exported item.

The exact assembled candidate is the resulting commit recorded in
[#209's completion/validation receipt](https://github.com/dshbox/cordis-rs/issues/209),
alongside its PR head, CI run and conclusion. This avoids an impossible self-hash
inside a versioned file. A later commit is a different candidate: recheck affected
claims and CI instead of inheriting a green result by branch name. Until the receipt
confirms final-head CI, the recommendation is conditional; after that verification,
no identified API freeze preparation blocker remains. This is not a 1.0 readiness
claim or a freeze announcement.

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

This register uses evidence and authority, not merely open-issue count. It was
checked after all eight path tickets closed; #209 has zero open native blockers
and eight total prerequisite edges. New material findings reopen review even if
those tickets remain closed.

| Item / disposition | Authority and evidence | Owner / dependency / resolution |
| --- | --- | --- |
| Supported-contract production defects: none identified | Eight source-to-contract paths and their named regressions in the evidence record | Maintainer; any newly proven defect blocks candidate selection until fixed and reviewed |
| Material missing discriminators: none identified | Public integration/UI tests plus explicitly bounded private kernel/projection/attribution probes | Maintainer; a newly identified material gap needs its actual reachable rival and a blocking ticket |
| Queued deliberate public breaks / accepted new renames: none | Member-level naming review, accepted ADRs and current tracker inspection | Author; a future approved rename gets its own ticket, migration impact and true blocking edges |
| D1–D3 documentation drift: resolved | Decision index/count and navigation; existing with_state export; six current runnable example sources | #201, #205, #206 and #209; no dependency remains after integration review |
| Exact final-head CI receipt: verification condition | CI workflow, local gate records and the #209 receipt; baseline CI is not final-head CI | CI/maintainer; inspect final commit/head/run, resolve only on required jobs passing, never infer success from a branch badge |
| Draft Wasm #145: excluded, not a queued break | [Experiment/wasm components](https://github.com/dshbox/cordis-rs/pull/145); author confirmed independent scope during planning | Author/experiment owner; no dependency on this candidate, future workspace/MSRV decisions remain separate |
| Broader state-space/performance expansion: independent follow-up | [Bounded concurrency evidence](lifecycle-concurrency-modeling.md), [performance baseline/policy](performance-benchmarking.md), ROADMAP confidence goals | Maintainer; deliberately outside this preparation, no newly invented semver gate |

The tracker check found only this preparation's parent and aggregate open, with
Wasm as the unrelated draft PR. That supports the queued-work check but cannot
prove absence of defects; the conformance review is the evidence for that finding.
There is no unresolved item deferred inside the supported contract to make the
candidate appear ready.

## Review and local validation chain

Every delivery pinned its starting commit, reviewed the complete diff on separate
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
| [#209](https://github.com/dshbox/cordis-rs/issues/209) | `753f534` | Exact result in completion receipt | Separate final integration reports in receipt | Eight final local gates; `freeze-209-20260928-01` |

Each run covers toolchain, fmt, clippy, vocabulary, tests, docs, examples and
floating-latest compatibility. Cached dependencies were used with
CARGO_NET_OFFLINE=true; canonical UI snapshots used the pinned Rust 1.98.1 and
rust-src. Latest checks compatibility only. Planning's initial DNS failure was
resolved by rerunning only its failed test gate offline; it was not a code failure.
No implementation ticket needed that retry. Gate logs are local artifacts; ticket
receipts retain the summaries even if routine target cleanup removes older logs.

The source baseline's [CI run](https://github.com/dshbox/cordis-rs/actions/runs/36353799979)
passed MSRV 1.88, dependency audit/policy, canonical tests on Linux/macOS/Windows,
Loom models, latest stable, formatting/Clippy/docs/vocabulary, all six examples
and release-package lanes. Final-candidate CI must be read separately from the
#209 receipt. Local gates do not duplicate the explicit MSRV, audit/policy, Loom
or release-package jobs; none is silently replaced by a local green test.

## Residual limits and later 1.0 work

Tests and selected bounded models do not establish universal equivalence,
every scheduling permutation or unbounded progress. Best-effort observations
are not an audit journal, snapshots are not globally linearizable, exact realms
have no fallback, Drop is inert, teardown is explicit, caller Event/Timeout work
stays caller-owned and external driver lifetime/process termination limits remain.
These are accepted authority boundaries rather than unfinished API promises.

[ROADMAP](../ROADMAP.md#required-before-10) still requires the author to select
and declare the candidate; then complete a stabilization release without a planned
break. A deliberate break updates authority and restarts that stabilization
requirement. Before 1.0, finish stable-promise discovery wording, the final
pre-1.0-to-1.0 migration note (or explicit no-change statement), and CI/package
verification for the actual release commit. Current pre-1.0 status wording and
ROADMAP checkboxes remain unchanged. This effort neither performs those releases
nor expands performance measurements.
