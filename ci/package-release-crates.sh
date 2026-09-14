#!/usr/bin/env bash
set -euo pipefail

# release-plz bumps workspace package versions together before any of them exist
# in crates.io. Verify core against its registry dependencies, then verify each
# leaf package against the exact workspace core that will be published first.
# The packaged manifests still carry their semver requirement; this patch only
# supplies that not-yet-published version during pre-release verification.
core_patch=(--config 'patch.crates-io.cordis-core.path="crates/cordis-core"')

cargo package --locked -p cordis-core
cargo package --locked -p cordis-timer "${core_patch[@]}"
cargo package --locked -p cordis-loader "${core_patch[@]}"
cargo package --locked -p cordis-rs "${core_patch[@]}"
