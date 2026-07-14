#!/usr/bin/env bash
#
# Assert the four places a version is written stay in lockstep:
#
#   crates/zed-http-core/Cargo.toml
#   crates/zed-http-cli/Cargo.toml
#   extension/extension.toml
#   CHANGELOG.md   (a released heading, `## [x.y.z] - <date>`)
#
# They are bumped by hand, so nothing but a check stops them drifting.
#
# Usage:
#   scripts/check-versions.sh          # the four must agree
#   scripts/check-versions.sh v0.5.0   # ...and must equal this tag
#
set -euo pipefail

cd "$(dirname "$0")/.."

# The first `version = "..."` in a manifest is the [package] one; the
# dependency versions that follow must not be picked up instead.
first_version() {
  local file="$1"
  local value
  value="$(grep -m1 -E '^version *= *"[^"]+"' "$file" | sed -E 's/.*"([^"]+)".*/\1/')"
  if [[ -z "$value" ]]; then
    echo "no version found in $file" >&2
    exit 1
  fi
  printf '%s' "$value"
}

core="$(first_version crates/zed-http-core/Cargo.toml)"
cli="$(first_version crates/zed-http-cli/Cargo.toml)"
extension="$(first_version extension/extension.toml)"

echo "zed-http-core       $core"
echo "zed-http-cli        $cli"
echo "extension.toml      $extension"

status=0

if [[ "$core" != "$cli" || "$core" != "$extension" ]]; then
  echo "::error::crate and extension versions disagree" >&2
  status=1
fi

# `## [Unreleased]` does not count: a release must have a real heading.
if ! grep -qE "^## \[${core}\]" CHANGELOG.md; then
  echo "::error::CHANGELOG.md has no '## [${core}]' heading" >&2
  status=1
else
  echo "CHANGELOG.md        ## [${core}]"
fi

# On a tag push, the tag itself is the fifth place the version appears.
if [[ $# -ge 1 ]]; then
  tag="${1#v}"
  echo "git tag             $tag"
  if [[ "$tag" != "$core" ]]; then
    echo "::error::tag '$1' does not match the crate version '$core'" >&2
    status=1
  fi
fi

if [[ $status -eq 0 ]]; then
  echo "OK: versions are in lockstep"
fi
exit $status
