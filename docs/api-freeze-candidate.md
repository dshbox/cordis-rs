# API freeze candidate evidence

Status: active evidence for the Cordis 1.0 readiness gate

The normative contract remains [the v3 public-interface inventory](v3-public-interface.md), with compatibility rules owned by [the compatibility policy](compatibility-policy.md). This document records why the repository can declare that inventory the candidate 1.0 supported surface; it does not add API declarations.

## Freeze baseline

The freeze begins at the first repository revision that contains both the candidate declaration in `docs/v3-public-interface.md` and the completed `Declare an API freeze candidate` ROADMAP criterion. Releases before that revision are pre-freeze and cannot satisfy the required post-freeze stabilization release.

At this baseline, the audit found **no known queued consumer-visible breaking change** to the supported surface. It did find and correct one inventory-completeness defect: the already-public and already-documented `event::with_state` adapter was missing from the canonical Event-module item table. That correction records the existing supported surface; it does not add or change Rust API. The declaration does not promise that the implementation is defect-free and does not turn unsupported or internal implementation details into supported API.

## Sources audited

The freeze review checked the remaining required ROADMAP criteria; the complete public-interface inventory, including explicit absences and unsupported boundaries; the compatibility policy; accepted ADRs and their falsifiers or future conditions; repository TODO/FIXME/deprecation/temporary/breaking-change signals; current public exports, Cargo features, error families, traits, and the Loader/Timer sibling seam; current open GitHub issues and PRs; and the rationale from the macro-policy, pre-freeze interface-reconciliation, and failure-maturity work.

The classifications that matter to the freeze are:

- **No queued breaking change:** no audited source records a supported declaration or behavior that the project already plans to remove, rename, move, or incompatibly redefine before 1.0.
- **Compatible additive work:** the possible future macro described by the interface is explicitly additive and conditional on demonstrated usage. Other genuinely new capabilities may likewise be added without reopening settled semantics when they obey the compatibility policy.
- **Explicit non-blockers and confidence work:** deeper concurrency exploration, independent review, bus-factor work, legacy companion redesigns, benchmark-harness choices, and similar ROADMAP items are not semver gates.
- **Closed designs with concrete reopening conditions:** Runtime-wide/structured ownership remains absent. ADR 0028 supplies its falsifier: core would need an ownership relation if it must collectively drain an admitted child before its `FiberHandle` is delivered. Until a demonstrated requirement meets that condition, the absence is settled rather than queued work.
- **Unsupported/internal surface:** `cordis-core/internal-api` and `cordis_core::__internal` remain unsupported downstream even when feature unification makes them reachable. Their use by published Loader/Timer siblings is nevertheless a package-compatibility obligation under ADR 0040 and the compatibility policy; reachability does not freeze the implementation seam as application API.
- **Experimental proposals:** an open draft Wasm-components proposal is additive, outside the current normative v3 supported surface, and explicitly unresolved as a normal published-workspace dependency. It is not a planned break to the frozen candidate surface.

The remaining required ROADMAP work is not evidence of a queued break. Making the stable promise discoverable is documentation work; the final migration note is produced after stabilization evidence exists; and the release-candidate/packaging gate validates the eventual release. None requires changing the candidate public contract merely to complete the criterion.

## Post-freeze change discipline

After the freeze, review changes against the compatibility policy and the candidate inventory.

Compatible additions, internal refactors, cleanup, documentation improvements, and correctness/security fixes that restore already-required behavior do **not** restart the stabilization-release count. A deliberate incompatible change to the candidate supported surface does. A correctness or security finding also restarts the count when fixing it requires changing the candidate contract incompatibly rather than merely correcting the implementation.

When a reset is required, the break and its owning authority must land together. The new baseline is the revision containing that corrected contract, and only a later real release can count as the required stabilization release.
