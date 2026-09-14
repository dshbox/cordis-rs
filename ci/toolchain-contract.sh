#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

fail() {
  printf 'toolchain-contract: %s\n' "$*" >&2
  exit 1
}

canonical="$(sed -n 's/^channel = "\([^"]*\)"$/\1/p' rust-toolchain.toml)"
[ -n "$canonical" ] || fail 'rust-toolchain.toml has no channel'
printf '%s\n' "$canonical" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' \
  || fail "canonical Rust must be an exact patch release, got: $canonical"

grep -Fq '"rust-src"' rust-toolchain.toml \
  || fail 'rust-toolchain.toml must install rust-src for trybuild diagnostics'

setup_default="$(awk '
  /^  toolchain:/ { in_toolchain=1; next }
  in_toolchain && /^    default:/ {
    gsub(/^[[:space:]]*default:[[:space:]]*/, "")
    gsub(/"/, "")
    print
    exit
  }
' .github/actions/setup-rust/action.yml)"
[ "$setup_default" = "$canonical" ] \
  || fail "setup-rust default ($setup_default) != rust-toolchain.toml ($canonical)"

grep -Fq 'RUSTUP_TOOLCHAIN=${{ inputs.toolchain }}' .github/actions/setup-rust/action.yml \
  || fail 'setup-rust must export RUSTUP_TOOLCHAIN so lane selection beats directory overrides'

grep -Fq 'toolchain: 1.88.0' .github/workflows/ci.yml \
  || fail 'CI must keep an explicit Rust 1.88.0 MSRV lane'
grep -Fq 'toolchain: stable' .github/workflows/ci.yml \
  || fail 'CI must keep a floating latest-stable compatibility lane'
grep -Fq 'components: rust-src' .github/workflows/ci.yml \
  || fail 'canonical test lane must install rust-src'
grep -Fq 'cargo test --locked --workspace' .github/workflows/ci.yml \
  || fail 'canonical test lane must run the locked workspace tests'
grep -Fq 'cargo check --locked --workspace --all-targets' .github/workflows/ci.yml \
  || fail 'CI must compile all targets on non-canonical compatibility lanes'

if grep -R -n -F 'uses: dtolnay/rust-toolchain@' .github/workflows >/dev/null; then
  fail 'workflows must select Rust through .github/actions/setup-rust, not bypass it'
fi

grep -Fq 'uses: ./.github/actions/setup-rust' .github/workflows/release-plz.yml \
  || fail 'release workflow must use the canonical setup-rust action'

printf 'toolchain-contract: canonical=%s, msrv=1.88.0, latest=stable\n' "$canonical"
