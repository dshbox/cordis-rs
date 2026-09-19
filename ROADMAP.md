# Cordis 1.0 readiness roadmap

Status: active release-readiness contract — no calendar commitment

## Purpose and authority

This roadmap answers one question:

> What must be true before Cordis calls the v3 release family 1.0?

It is a release-readiness checklist, not a feature roadmap and not an architecture
source. It does not define Runtime semantics, public API declarations, lifecycle
rules, or migration behavior. Those remain owned by the existing authorities:

- [`CONTEXT.md`](CONTEXT.md) owns canonical domain vocabulary.
- [`docs/v3-architecture.md`](docs/v3-architecture.md) owns normative v3
  architecture and invariants.
- [`docs/v3-public-interface.md`](docs/v3-public-interface.md) owns the exhaustive
  supported caller-visible contract.
- Accepted ADRs linked from the architecture document own hard-to-reverse design
  decisions and rationale.
- [`docs/v3-upstream-parity-ledger.md`](docs/v3-upstream-parity-ledger.md) owns
  upstream relationship claims.

[`docs/v3-migration.md`](docs/v3-migration.md) remains non-normative migration and
test evidence. [`docs/lifecycle-concurrency-modeling.md`](docs/lifecycle-concurrency-modeling.md)
is the durable evidence boundary for systematic lifecycle-concurrency validation;
it is not a new architecture authority.

A roadmap checkbox may summarize evidence from those documents, but it cannot
change their contract. If this file conflicts with a normative source, the
normative source wins and this checklist must be corrected.

The checklist gates a project-level 1.0 claim. It does not decide whether the
application-facing `cordis-rs` facade and the semantic crates adopt 1.0
simultaneously; the compatibility/release policy must make package-scoped stability
promises explicit.

## Readiness already established

- [x] **The v3 architecture and public surface have a closed authority set.** The
  architecture names the normative reading closure, and the public-interface
  inventory is exhaustive with no remaining semantic frontier.
- [x] **Critical lifecycle concurrency has systematic evidence.** The lifecycle
  concurrency plan has completed Phases 1–3, including reduced Loom models,
  historical negative controls, real Tokio contracts, Era ownership/cancellation
  evidence, cross-protocol convergence evidence, and the required PR CI matrix.
  This is bounded correctness evidence, not a mathematical proof or a claim of
  unbounded liveness.
- [x] **Compiler and platform compatibility are explicit and continuously checked.**
  The workspace declares MSRV Rust 1.88, pins a canonical development/UI toolchain,
  checks the MSRV and floating latest stable, and runs canonical tests on Linux,
  macOS, and Windows.
- [x] **Release mechanics are exercised.** CI checks formatting, Clippy, docs,
  dependency policy, examples, packageability, and workspace tests; release-plz
  maintains release PRs and publishes only through the release workflow.
- [x] **Migration and consumer documentation exist for the v3 transition.** The
  repository documents the 0.6.x-to-v3 migration, the 0.7.x-to-0.8.x naming break,
  the supported public surface, and runnable consumer examples.
- [x] **Failure and cancellation semantics are already first-class contracts.** The
  public-interface inventory defines operation-specific error families, panic
  boundaries, cancellation laws, commit ownership, and cleanup obligations rather
  than collapsing them into one global error model.

These completed items are evidence to preserve. They are not invitations to reopen
settled architecture merely to create more pre-1.0 work.

## Required before 1.0

### Public API stability

- [x] **Publish the 1.0 compatibility policy.** The
  [compatibility policy](docs/compatibility-policy.md) defines the supported
  compatibility surface, breaking-change rules, deprecation/removal policy,
  Cargo feature expectations, package-version coordination, and MSRV changes.
  `internal-api` remains unsupported downstream while its use by already-published
  sibling crates carries an explicit package-compatibility obligation.
- [ ] **Declare an API freeze candidate.** Resolve every known planned breaking
  change before the freeze and record that the normative public-interface inventory
  is the candidate 1.0 surface.
- [x] **Resolve public macro ergonomics before the API freeze.** Representative
  declarations were reviewed in `hello_plugin`, `scopes_tenants`, `gateway`, and
  `chat_capstone` against the public trait contracts. Cordis 1.0 intentionally
  ships with no public declarative or procedural macro surface:
  direct implementation of the semantic traits is the canonical authoring style,
  as recorded in the normative
  [`v3 public interface`](docs/v3-public-interface.md#authoring-style-and-macro-surface).
  `Service` and `Event` declarations keep small semantic contracts explicit, while
  `Plugin` and `ConfigurableService` bodies contain behavior that macro syntax would
  not eliminate. A future macro remains possible as additive supported API when a
  demonstrated repeated pattern has one unambiguous semantic meaning; generated
  representation details are not thereby part of the stable contract.
- [ ] **Complete one stabilization release after the freeze without a planned
  breaking change.** If a deliberate public break is required, make the break,
  update its owning authority, and restart the stabilization release requirement.
  Correctness or security fixes remain mandatory even when they expose that the
  candidate contract was wrong.
- [ ] **Cut 1.0 with no known queued breaking change.** The release decision must
  not knowingly publish a surface that the project already intends to replace.

### Performance regression evidence

- [x] **Establish a representative benchmark baseline.** The public-path suite and
  measured/reset boundaries are recorded in
  [`docs/performance-benchmarking.md`](docs/performance-benchmarking.md). It measures
  Cordis overhead separately from arbitrary Plugin or consumer callback work and
  covers spawn plus initial settle; already-quiescent `ready()`; Event emit/query
  with 0, 1, and N listeners; Service publication/visibility mutation affecting 0,
  1, and N Fibers; `restart`; `update`; and `era_swap`.
- [x] **Document how regressions are reviewed.** The benchmark evidence document
  records the environment, sampling/variance method, observed noise, and a 15%
  review trigger with same-machine confirmation. A threshold crossing triggers
  human review and explanation, not an automatic correctness or CI failure.
- [x] **Record an initial baseline suitable for future comparison.** The benchmark
  evidence document records the first local reference and its three-run noise span.
  The numbers are regression evidence, not proof that Cordis is "production ready"
  and not a marketing performance target.

### Operational and failure maturity

- [x] **Close a 1.0 failure-boundary audit.** The
  [failure-boundary audit](docs/failure-boundary-audit.md) maps every supported
  critical-path family across returned failure, contained panic, caller
  cancellation, executor-unavailable/off-runtime behavior where applicable,
  cross-task lifecycle-recursion attribution, deterministic cleanup, and
  application teardown. Every matrix cell is classified as Covered, N/A, or
  explicitly Unsupported, with no remaining material gap.
- [x] **Verify teardown guidance against demonstrated consumers.** The top-level
  README and [application teardown guide](docs/application-teardown.md) document
  the demonstrated Harness/Roster policy: retain delivered `FiberHandle`s,
  dispose them in reverse spawn order, and attempt every disposal. The shared
  `examples_common::Roster` and `examples/common/tests/boot.rs` exercise
  reverse-order, attempt-all, and repeat teardown behavior. No Runtime-wide
  shutdown API was added; the approved architecture's absence and reopening
  condition remain unchanged.

### Documentation and release policy

- [ ] **Make the stable promise discoverable.** Before 1.0, replace the generic
  pre-1.0 warning with links to the compatibility policy and the stable public
  contract, including the unsupported `internal-api` boundary.
- [ ] **Publish a final pre-1.0-to-1.0 migration note.** It must enumerate any
  consumer-visible changes since the stabilization release, or explicitly state
  that there are none, and point to the normative interface rather than duplicating
  it.
- [ ] **Run the release candidate through the repository's required CI and packaging
  gates.** The 1.0 tag must be cut from a commit that satisfies the same toolchain,
  platform, dependency-policy, examples, tests, docs, and packageability contracts
  used for ordinary releases.

## Confidence goals that are not semver gates

The following work can improve confidence before or after 1.0, but its mere absence
must not keep the project permanently pre-1.0:

- Maintain the short
  [external concurrency review guide](docs/lifecycle-concurrency-review-guide.md) that
  points reviewers at named invariants, forbidden states, transition/linearization
  points, reduced models, negative controls, real Tokio evidence, and known coverage
  boundaries. An independent critical-path review is strongly desirable; any correctness
  finding it produces becomes a blocker until resolved, but obtaining a particular
  reviewer is not itself a release gate.
- Continue scheduled/deeper concurrency assurance such as larger actor counts,
  higher exploration/preemption bounds, deeper Era failure/cancellation combinations,
  and optional Shuttle random/PCT runs. Passing bounded exploration must continue to
  be described as bounded evidence, never formal proof.
- Reduce bus factor through review participation, documentation, and maintainership
  over time. A second maintainer is desirable project health, not a semantic-version
  prerequisite.

## Explicit non-blockers

The following are not required merely to call the current v3 contract 1.0:

- a v3-native `cordis-cli`, `cordis-include`, or `cordis-group`; those legacy
  companion crates remain on the 0.6.x line until a v3-native consumer need justifies
  a new design;
- a Runtime-wide shutdown API, structured Fiber owner, or other capability the
  approved architecture deliberately excludes behind a concrete reopening condition;
- deeper Loom/Shuttle exploration beyond the completed declared ranges;
- a second maintainer or named external reviewer;
- a particular benchmark library or harness, a headline throughput target, or
  performance numbers used as a proxy for correctness; and
- calendar milestones, feature-count targets, or marketing launch commitments.

## Exit rule

Cordis is ready to call the v3 release family 1.0 when every checkbox under
**Required before 1.0** is complete and the release candidate has no known unresolved
correctness defect in the supported contract or queued deliberate breaking change.
Confidence goals may remain open. A new finding that contradicts a normative v3
contract reopens the relevant required criterion until the contract or implementation
is corrected through its normal authority and review process.
