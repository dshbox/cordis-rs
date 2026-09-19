#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

# release-plz bumps workspace package versions together before the new core
# version exists in crates.io. Verify packages against the exact workspace core
# that will be published first. Packaged manifests still carry their normal
# semver requirements; this patch only supplies that not-yet-published core
# during pre-release verification.
core_patch=(--config 'patch.crates-io.cordis-core.path="crates/cordis-core"')

cargo package --locked -p cordis-core
cargo package --locked -p cordis-timer "${core_patch[@]}"
cargo package --locked -p cordis-loader "${core_patch[@]}"
cargo package --locked -p cordis-rs "${core_patch[@]}"
