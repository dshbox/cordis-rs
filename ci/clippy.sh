#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

cargo clippy --locked --workspace --all-targets -- -D warnings

# cordis-loader enables internal-api in the workspace check. Lint core
# separately with its default features and with the Loom test targets.
cargo clippy --locked -p cordis-core --all-targets -- -D warnings
cargo clippy --locked -p cordis-core --all-targets --features loom-tests -- -D warnings
