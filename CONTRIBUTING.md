# Contributing to Cordis

Thanks for contributing. Cordis is pre-1.0 and its runtime semantics are documented deliberately, so changes should preserve the repository's existing contracts rather than introduce parallel terminology or policy.

## Before changing code

Read the repository guidance relevant to the change:

- `AGENTS.md` for the repository workflow and required gates;
- `CONTEXT.md` for canonical domain vocabulary;
- `CODING_STANDARDS.md` for implementation conventions;
- `docs/adr/` for decisions that constrain the area you are changing.

For substantial behavior or API changes, open an issue first so the contract can be discussed before implementation. Bug fixes and narrowly scoped documentation or maintenance changes may go directly to a pull request when the intent is already clear.

## Development

The exact development toolchain is pinned in `rust-toolchain.toml`. From the repository root, run the local gate runner before committing:

```bash
ci/gates.sh <run-id>
```

Use a run id unique to your work, such as an issue number or short feature slug. The runner executes the repository's formatting, Clippy, vocabulary, test, documentation, example, and compatibility checks and writes reusable logs under `target/gates/<run-id>/`. Do not rerun a failed gate just to inspect it; read the saved log first.

CI additionally owns the explicit MSRV, dependency audit/policy, dedicated Loom model, and release-package lanes.

## Pull requests

Keep a pull request focused on one coherent change. Update user-facing documentation when behavior or public API changes, preserve existing vocabulary from `CONTEXT.md`, and call out any intentional ADR conflict explicitly.

Before requesting review, make sure `ci/gates.sh <run-id>` passes locally and include any validation that cannot be represented by the standard gates in the pull request description.
