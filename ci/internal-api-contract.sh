#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
repo_root="$PWD"
workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT
target_dir="$repo_root/target/internal-api-contract"
rm -rf "$target_dir"
mkdir -p "$target_dir"

make_case() {
  local name="$1"
  mkdir -p "$workdir/$name/src"
}

make_case default-core
cat >"$workdir/default-core/Cargo.toml" <<EOF
[package]
name = "cordis-internal-default-probe"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
cordis-core = { path = "$repo_root/crates/cordis-core" }
EOF
cat >"$workdir/default-core/src/main.rs" <<'EOF'
fn main() {
    let _ = cordis_core::__internal::generation_cleanup_admitted;
}
EOF
if CARGO_TARGET_DIR="$target_dir" cargo check --quiet --manifest-path "$workdir/default-core/Cargo.toml" >"$workdir/default-core.log" 2>&1; then
  echo "internal-api-contract: default cordis-core unexpectedly exposed __internal" >&2
  exit 1
fi
grep -q '__internal' "$workdir/default-core.log" || {
  cat "$workdir/default-core.log" >&2
  echo "internal-api-contract: default-core failure was unrelated to __internal" >&2
  exit 1
}

make_case sibling-unification
cat >"$workdir/sibling-unification/Cargo.toml" <<EOF
[package]
name = "cordis-internal-sibling-probe"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
cordis-core = { path = "$repo_root/crates/cordis-core" }
cordis-timer = { path = "$repo_root/crates/cordis-timer" }
cordis-loader = { path = "$repo_root/crates/cordis-loader" }
EOF
cat >"$workdir/sibling-unification/src/main.rs" <<'EOF'
fn main() {
    let _ = cordis_core::__internal::generation_cleanup_admitted;
    cordis_core::__internal::detach_completion(async {});
}
EOF
CARGO_TARGET_DIR="$target_dir" cargo check --quiet --manifest-path "$workdir/sibling-unification/Cargo.toml"

make_case facade-boundary
cat >"$workdir/facade-boundary/Cargo.toml" <<EOF
[package]
name = "cordis-internal-facade-probe"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
cordis-rs = { path = "$repo_root/crates/cordis" }
cordis-timer = { path = "$repo_root/crates/cordis-timer" }
cordis-loader = { path = "$repo_root/crates/cordis-loader" }
EOF
cat >"$workdir/facade-boundary/src/main.rs" <<'EOF'
fn main() {
    let _ = cordis::__internal::generation_cleanup_admitted;
}
EOF
if CARGO_TARGET_DIR="$target_dir" cargo check --quiet --manifest-path "$workdir/facade-boundary/Cargo.toml" >"$workdir/facade-boundary.log" 2>&1; then
  echo "internal-api-contract: cordis facade unexpectedly re-exported __internal" >&2
  exit 1
fi
grep -q '__internal' "$workdir/facade-boundary.log" || {
  cat "$workdir/facade-boundary.log" >&2
  echo "internal-api-contract: facade failure was unrelated to __internal" >&2
  exit 1
}

echo "internal-api-contract: default hidden, sibling-unified direct core reachable, facade closed"
