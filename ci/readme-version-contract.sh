#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

fail() {
  printf 'readme-version-contract: %s
' "$*" >&2
  exit 1
}

workspace_version="$(sed -n 's/^version = "\([0-9][0-9.]*\)"$/\1/p' Cargo.toml | head -n 1)"
[ -n "$workspace_version" ] || fail 'Cargo.toml has no workspace package version'
semantic_minor="${workspace_version%.*}"
semantic_line="${semantic_minor}.x"

facade_version="$(sed -n 's/^version = "\([0-9][0-9.]*\)"$/\1/p' crates/cordis/Cargo.toml | head -n 1)"
[ -n "$facade_version" ] || fail 'crates/cordis/Cargo.toml has no package version'
facade_minor="${facade_version%.*}"
facade_line="${facade_minor}.x"

require_text() {
  local file="$1"
  local expected="$2"
  grep -Fq "$expected" "$file" || fail "$file must contain: $expected"
}

require_match() {
  local file="$1"
  local expected="$2"
  grep -Eq "$expected" "$file" || fail "$file must match: $expected"
}

for file in README.md README.zh-CN.md crates/cordis-core/README.md crates/cordis-loader/README.md crates/cordis-timer/README.md; do
  require_text "$file" "cordis-core = \"$semantic_minor\""
  require_text "$file" "cordis-rs = \"$facade_minor\""
done
require_text README.md "cordis-timer = \"$semantic_minor\""
require_text README.md "cordis-loader = \"$semantic_minor\""
require_text README.zh-CN.md "cordis-timer = \"$semantic_minor\""
require_text README.zh-CN.md "cordis-loader = \"$semantic_minor\""
require_text crates/cordis-loader/README.md "cordis-loader = \"$semantic_minor\""
require_text crates/cordis-timer/README.md "cordis-timer = \"$semantic_minor\""

require_match crates/cordis-core/README.md "cordis-timer.*${semantic_line}"
require_match crates/cordis-core/README.md "cordis-loader.*${semantic_line}"
require_match crates/cordis-core/README.md "now publish on .*${semantic_line}"
require_match crates/cordis-loader/README.md "publishes on .*${semantic_line}"
require_match crates/cordis-loader/README.md "current .*${semantic_line}.*line depends on"
require_match crates/cordis-loader/README.md "cordis-core ${semantic_line}"
require_match crates/cordis-timer/README.md "current .*${semantic_line}.*line depends on"
require_match crates/cordis-timer/README.md "cordis-core ${semantic_line}"

require_match README.md "cordis-rs.*${facade_line}"
require_match README.zh-CN.md "cordis-rs.*${facade_line}"
require_match crates/cordis-core/README.md "cordis-rs.*${facade_line}"

printf 'readme-version-contract: semantic=%s facade=%s\n' "$semantic_line" "$facade_line"
