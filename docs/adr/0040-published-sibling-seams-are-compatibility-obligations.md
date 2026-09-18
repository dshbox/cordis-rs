# Published sibling seams are compatibility obligations

Status: accepted

`cordis-core/internal-api` is not supported downstream API, even when Cargo
feature unification makes its doc-hidden `cordis_core::__internal` module
reachable. However, `cordis-timer` and `cordis-loader` are published crates that
consume that seam through ordinary Cargo-compatible `cordis-core` requirements.
Every core release admitted by an already-published sibling requirement must
therefore preserve the sibling-facing seam needed by that release.

We keep the seam explicitly unsupported for downstream consumers while treating
its sibling-used signatures and semantics as a package compatibility obligation.
Breaking the seam requires a coordinated semver-incompatible semantic-line
release unless old published requirements already exclude the new core. Exact
pinning or artificially narrowing `cordis-core` merely to hide the seam is
rejected: it can resolve multiple core copies in one application and split
`Context` and other Cordis type identity. A future refactor may remove the seam
only by replacing it with a genuinely better semantic boundary, not by adding a
shallow public wrapper with the same coupling.
