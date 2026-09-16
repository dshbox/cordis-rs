#!/usr/bin/env bash
# gates: the eight local pre-commit checks, including the floating-stable
# compatibility check. CI separately provisions the explicit MSRV compile,
# dependency audit/policy, dedicated Loom model, and release-package lanes;
# this runner does not install their extra tools or duplicate those jobs.
# Run it before committing; ask follow-up questions of a gate's log
# instead of re-running the gate.
#
# The first argument is an optional run-id naming the log directory
# target/gates/<run-id>/ — pass one per working session (ticket number
# or slug) so every invocation in that session writes to the same
# directory and the logs stay unique across parallel sessions. Reusing
# a run-id overwrites only the gates re-run, never other logs. Without
# a run-id, timestamp.pid is generated (unique, but one directory per
# invocation). target/gates/latest points at the most recent run; runs
# older than the newest 10 are pruned.
#
# Test and example runs carry the same timeout backstop CI uses, so a
# hang fails as exit 124 instead of blocking the run. GATES_TEST_TIMEOUT
# (seconds, default 300) overrides the test budget.
#
# Usage: ci/gates.sh [run-id] [gate ...]   # gates default: all eight
set -u

cd "$(dirname "$0")/.."

# Local gates are reproducible even when the caller has an ambient rustup
# override. The `latest` gate opts back into the floating stable alias for its
# compatibility-only compile check.
canonical_rust="$(sed -n 's/^channel = "\([^"]*\)"$/\1/p' rust-toolchain.toml)"
[ -n "$canonical_rust" ] || { echo 'gates: missing rust-toolchain.toml channel' >&2; exit 1; }
export RUSTUP_TOOLCHAIN="$canonical_rust"

logdir=target/gates
mkdir -p "$logdir"

gates=("$@")
run_id=
# First arg names the run unless it is itself a gate.
if [ $# -gt 0 ]; then
  case $1 in
    toolchain|fmt|clippy|vocab|test|doc|examples|latest) ;;
    *) run_id=$1; shift; gates=("$@") ;;
  esac
fi
[ $# -eq 0 ] && gates=(toolchain fmt clippy vocab test doc examples latest)
[ -n "$run_id" ] || run_id="$(date +%Y%m%d-%H%M%S).$$"
# The run-id is a path component under target/gates/ — keep it one.
case $run_id in
  .*|*[!A-Za-z0-9._-]*)
    printf 'gates: invalid run-id: %s (use [A-Za-z0-9._-], no leading dot)\n' "$run_id" >&2
    exit 2
    ;;
esac

run_dir="$logdir/$run_id"
mkdir -p "$run_dir" || { printf 'gates: cannot create %s\n' "$run_dir" >&2; exit 1; }
# Reused run-ids must not age out mid-session: dir mtime marks the run.
touch "$run_dir"
ln -sfn "$run_id" "$logdir/latest"
ls -1dt "$logdir"/*/ 2>/dev/null | grep -v '/latest/$' | tail -n +11 | xargs -r rm -rf
printf 'run %s — logs in %s\n' "$run_id" "$run_dir"

gate_toolchain() { ci/toolchain-contract.sh; }

gate_fmt()    { cargo fmt --all --check; }

gate_clippy() { cargo clippy --locked --workspace --all-targets -- -D warnings; }

gate_vocab()  { ci/harness-vocab-scan.sh; }

gate_test()   { timeout --kill-after=5s "${GATES_TEST_TIMEOUT:-300}" cargo test --locked --workspace; }

gate_doc()    { ci/readme-version-contract.sh && RUSTDOCFLAGS='-D warnings' cargo doc --locked --workspace --no-deps; }

gate_latest() { RUSTUP_TOOLCHAIN=stable cargo check --locked --workspace --all-targets; }

gate_examples() {
  # Build first so the per-example timeout measures run time, not cold
  # compile (same rationale as the CI job). Examples are headless and
  # self-terminating; stdin stays closed so the gate never depends on a TTY.
  cargo build --locked --workspace || return
  # The example list stays hardcoded and hand-updated in ci.yml; deriving it
  # here keeps one source of truth. Deriving zero
  # names is an error — an empty list would pass the gate vacuously.
  local examples
  examples="$(sed -n 's/.*cargo run \(--locked \)\?-p \([a-z_]*\) *$/\2/p' .github/workflows/ci.yml)"
  if [ -z "$examples" ]; then
    echo 'gates: no examples found in .github/workflows/ci.yml' >&2
    return 1
  fi
  local ex
  for ex in $examples; do
    timeout --kill-after=5s 120 cargo run --locked -p "$ex" < /dev/null || return
  done
}

# run NAME FUNCTION — one gate, one run, one log; failures print the
# log's tail so the next question is answered without a re-run.
results=()
run() {
  local name=$1 fn=$2 log="$run_dir/$1.log"
  if "$fn" >"$log" 2>&1; then
    results+=("PASS $name")
    printf 'PASS %s (log: %s)\n' "$name" "$log"
  else
    local st=$?
    results+=("FAIL $name")
    printf 'FAIL %s (exit %d%s) — log: %s\n' "$name" "$st" \
      "$([ "$st" -eq 124 ] && printf ' — timed out')" "$log"
    tail -n 15 "$log" | sed 's/^/    /'
  fi
}

gates=("$@")
[ $# -eq 0 ] && gates=(toolchain fmt clippy vocab test doc examples latest)

for g in "${gates[@]}"; do
  case $g in
    toolchain) run toolchain gate_toolchain ;;
    fmt)      run fmt      gate_fmt ;;
    clippy)   run clippy   gate_clippy ;;
    vocab)    run vocab    gate_vocab ;;
    test)     run test     gate_test ;;
    doc)      run doc      gate_doc ;;
    examples) run examples gate_examples ;;
    latest)   run latest   gate_latest ;;
    *)
      printf 'gates: unknown gate: %s\n' "$g" >&2
      printf 'usage: ci/gates.sh [run-id] [gate ...]  # gates: toolchain fmt clippy vocab test doc examples latest\n' >&2
      exit 2
      ;;
  esac
done

pass=$(printf '%s\n' "${results[@]}" | grep -c '^PASS')
fail=$(printf '%s\n' "${results[@]}" | grep -c '^FAIL')

if [ "$fail" -eq 0 ]; then
  printf '==== gates: %d/%d PASS ====\n' "$pass" "${#results[@]}"
  exit 0
fi
printf '==== gates: %d/%d PASS, %d FAIL — logs in %s/ ====\n' \
  "$pass" "${#results[@]}" "$fail" "$run_dir"
exit 1
