#!/usr/bin/env bash
set -euo pipefail

# release-plz bumps workspace package versions together before any of them exist
# in crates.io. Verify each package against the exact workspace dependencies
# that will be published earlier in the same release. The packaged manifests
# still carry their semver requirements; these patches only supply those
# not-yet-published versions during pre-release verification.
core_patch=(--config 'patch.crates-io.cordis-core.path="crates/cordis-core"')
facade_patch=(
    "${core_patch[@]}"
    --config 'patch.crates-io.cordis-timer.path="crates/cordis-timer"'
)

cargo package --locked -p cordis-core
cargo package --locked -p cordis-timer "${core_patch[@]}"
cargo package --locked -p cordis-loader "${core_patch[@]}"
cargo package --locked -p cordis-rs "${facade_patch[@]}"
