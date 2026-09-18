# Compatibility policy

This policy defines the compatibility promise Cordis is preparing to make at
1.0. During the remaining pre-1.0 stabilization work, changes intended for the
1.0 line should already follow this policy unless the project deliberately
reopens the contract and restarts the stabilization requirement in
[ROADMAP.md](../ROADMAP.md).

The normative semantic interface remains
[`docs/v3-public-interface.md`](v3-public-interface.md). This document owns the
versioning and compatibility rules around that interface; it does not duplicate
its declarations.

## Supported compatibility surface

For `cordis-core`, `cordis-loader`, and `cordis-timer`, the supported downstream
surface is the public API and behavior recorded in the normative v3 public
interface, using the documented supported Cargo features. The application
package `cordis-rs` additionally supports its explicit `cordis` re-exports of
that semantic surface.

A supported contract includes more than names that happen to compile. It
includes documented signatures and trait bounds, operation results and error
families, ownership and cancellation laws, ordering guarantees, and other
caller-visible semantics named by the public-interface inventory.

Doc-hidden implementation details, private modules, test-only facilities, and
features explicitly classified below as unsupported are outside the downstream
compatibility promise even if Rust or Cargo makes them reachable in a particular
dependency graph.

## What counts as a breaking change

After 1.0, a change is breaking when it makes previously supported downstream
code or documented behavior invalid within the same major line. This includes,
without limitation:

- removing, renaming, or moving a supported item;
- changing a supported signature, required trait bound, or exhaustive enum in a
  source-incompatible way;
- changing documented ownership, cancellation, ordering, error, or lifecycle
  semantics incompatibly;
- removing or renaming a supported Cargo feature, or incompatibly changing what
  a supported feature means;
- changing the `cordis-rs` facade so a previously supported re-export no longer
  resolves; and
- publishing package constraints that falsely describe an incompatible Cordis
  package combination as compatible.

Bug fixes may tighten implementation behavior where the normative contract
already requires that behavior. If a correctness or security fix truly requires
breaking the supported contract, it must use a semver-incompatible release
rather than silently redefining the existing major line.

## Deprecation and removal

Deprecation is advisory; a supported deprecated API remains supported for the
rest of the current major line and is removed only in a semver-incompatible
release.

ADR 0037's one-canonical-path rule still applies. A pure rename or move is not
bridged by adding a second compatibility alias. If such a rename cannot be made
additively without creating two canonical spellings, it waits for the next
semver-incompatible release. Additive replacement APIs are acceptable only when
they are genuinely distinct supported operations rather than aliases for the
same declaration.

## Cargo feature compatibility

`cordis-core` currently has no supported opt-in downstream feature surface. Its
default-feature semantic API is the supported downstream contract.

`internal-api` is a non-default, doc-hidden implementation feature used by
published Cordis sibling crates. `loom-tests` is a test/CI feature. Neither is a
supported downstream feature, and downstream code must not rely on either name,
their contents, or their continued availability as application API.

Cargo feature unification can nevertheless enable `internal-api` for a direct
`cordis-core` dependency when the same graph also contains `cordis-timer` or
`cordis-loader`. In that graph, `cordis_core::__internal` can therefore be
reachable. Reachability does **not** convert it into supported downstream API,
and `cordis-rs` does not re-export it.

The sibling use of `internal-api` has a separate compatibility responsibility:
every released `cordis-core` version admitted by an already-published sibling's
dependency requirement must continue to compile the sibling seam that release
uses. A core change may not break an old published `cordis-timer` or
`cordis-loader` while Cargo still considers that core version compatible with
the leaf package.

Changing or removing the sibling seam therefore requires a coordinated
semver-incompatible semantic-line release unless the published dependency
requirements already exclude the new core version. Merely hiding the seam
behind a differently named shallow wrapper does not change this obligation.

## Package-version coordination

`cordis-core`, `cordis-timer`, and `cordis-loader` currently share the workspace
semantic version. `cordis-rs` keeps its historical, independently versioned
application-facade line. Shared version numbers are release coordination, not a
substitute for dependency compatibility: each published package requirement
still determines which combinations Cargo may resolve.

Published semantic siblings should use the normal Cargo-compatible requirement
for the core line they support. Do not exact-pin or artificially narrow
`cordis-core` merely to conceal `internal-api`. A narrow pin can cause an
application that also depends on another compatible-looking core release to
resolve two `cordis-core` copies. `Context` and other Cordis types from those
copies then have different Rust type identity, which can make extension traits
and cross-crate APIs unusable together.

When a sibling-facing core seam must break, coordinate the semantic packages so
the new leaf and core requirements describe a genuinely incompatible line. For
the current `0.3.x` semantic line, that means a break cannot masquerade as
another `0.3.x` patch. Once the stable line is `1.x`, the corresponding
incompatible change requires the next major line.

The `cordis-rs` facade may release independently, but each facade release must
name a compatible core requirement and must preserve its supported re-export
surface for the lifetime of that facade major line.

## MSRV changes

The workspace `rust-version` is the MSRV authority and CI must continue to
compile the whole supported workspace at that version.

Patch releases do not raise MSRV. After 1.0, an MSRV increase is permitted only
in a minor release, must update `rust-version`, the documented MSRV, and the MSRV
CI lane together, and must be called out in release notes. Raising MSRV is a
toolchain compatibility change even when it does not otherwise change the Rust
API.

## Release review consequences

Before release, compatibility review must distinguish the supported downstream
surface from the published-sibling seam:

- downstream review checks the normative public interface and facade re-exports;
- sibling review checks that every still-compatible published leaf can compile
  against the candidate core seam; and
- Cargo feature-unification tests must reflect reality: the internal module is
  absent from the ordinary default core surface, may become reachable when a
  sibling activates the feature, and still must not escape through `cordis-rs`.

An intentional break updates the owning authority and package compatibility
story in the same change. A test must never claim that feature unification makes
`cordis_core::__internal` unreachable when the package graph in fact enables it.
