# AGENTS.md

Guidance for agents working in this repo.

## Agent workflow conventions

### Issue tracker

Issues and specs are tracked as GitHub issues using the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The five canonical triage roles, with label strings equal to their names. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: one `CONTEXT.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.

## CI gates

Before committing, run `ci/gates.sh` — the repository's eight local
pre-commit gates (toolchain contract, fmt, clippy, vocabulary, tests, docs,
examples, and floating-latest compatibility), once each, with a PASS/FAIL
summary. CI additionally owns the explicit MSRV compile, dependency audit/policy,
dedicated Loom model, and release-package lanes; those lanes are not duplicated
by the default local runner, and the audit lane relies on CI-provisioned tools. Pass a run-id unique
to your session — ticket
number or feature slug, e.g. `ci/gates.sh t19 fmt` — so all your
invocations share one log directory (`target/gates/<run-id>/`) and never
collide with a parallel session's. Grep a gate's log instead of
re-running it.

### UI diagnostics

`trybuild` `.stderr` files are golden compiler-UI contracts. The exact Rust
patch release and `rust-src` component in `rust-toolchain.toml` are the
canonical snapshot authority; do not bless snapshots under an arbitrary
`stable` toolchain. `ci/toolchain-contract.sh` keeps local/CI toolchain
selection aligned. CI also compiles all targets on floating latest stable,
but that compatibility lane intentionally does not compare golden stderr.
When upgrading the canonical Rust version, review any snapshot refreshes as
part of that same deliberate toolchain-upgrade change.

## Conversation

Conversation with the author may be Chinese, but keep
software-engineering vocabulary in its English spelling — ticket, map,
claim, resolve, frontier, ADR, review, commit, probe. Translated forms
(e.g. 票 for ticket) read wrong in a dev context.

<!-- CODEGRAPH_START -->
## CodeGraph

In repositories indexed by CodeGraph (a `.codegraph/` directory exists at the repo root), reach for it BEFORE grep/find or reading files when you need to understand or locate code:

- **MCP tool** (when available): `codegraph_explore` answers most code questions in one call — the relevant symbols' verbatim source plus the call paths between them, including dynamic-dispatch hops grep can't follow. Name a file or symbol in the query to read its current line-numbered source. If it's listed but deferred, load it by name via tool search.
- **Shell** (always works): `codegraph explore "<symbol names or question>"` prints the same output.

If there is no `.codegraph/` directory, skip CodeGraph entirely — indexing is the user's decision.
<!-- CODEGRAPH_END -->
