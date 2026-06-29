#!/usr/bin/env bash
# Feature 010 — release-time pre-flight version-consistency check (FR-009).
#
# Asserts the git tag (minus leading `v`) equals all three in-repo version
# strings:
#   - Cargo.toml workspace `version`
#   - charts/mikebom-operator/Chart.yaml `version`
#   - charts/mikebom-operator/Chart.yaml `appVersion`
#
# Exits non-zero with a clear diff on mismatch. Invoked by the `versions` job
# in .github/workflows/release.yml.

set -euo pipefail

TAG="${GITHUB_REF_NAME:-}"
if [ -z "$TAG" ]; then
  echo "ERROR: GITHUB_REF_NAME is not set; this script runs only in CI on a tag push" >&2
  exit 2
fi

# Strip leading `v` from the tag (e.g., v0.1.0-alpha.1 -> 0.1.0-alpha.1).
EXPECTED="${TAG#v}"
if [ "$EXPECTED" = "$TAG" ]; then
  echo "ERROR: tag '$TAG' does not start with 'v'; release tags MUST be of form vMAJOR.MINOR.PATCH[-PRERELEASE]" >&2
  exit 1
fi

# Parse Cargo.toml workspace version.
#   form: version = "X.Y.Z[-pre]"  — appears under [workspace.package]
CARGO_VERSION="$(awk '
  /^\[workspace\.package\]/ { in_section = 1; next }
  /^\[/                     { in_section = 0; next }
  in_section && /^version[[:space:]]*=/ {
    gsub(/^[^"]*"/, "")
    gsub(/".*$/, "")
    print
    exit
  }
' Cargo.toml)"

if [ -z "$CARGO_VERSION" ]; then
  echo "ERROR: failed to parse [workspace.package] version from Cargo.toml" >&2
  exit 1
fi

# Parse Chart.yaml version + appVersion via yq.
#   yq v4: `yq '.version'` / `yq '.appVersion'`
#   yq v3 (Python): `yq -r '.version'` (also accepts the v4 syntax for simple paths)
# We assume `yq` v4 (Go) is installed by the workflow's `mikefarah/yq` action OR
# `pip install yq` (Python, jq-based). Both expose `.version` / `.appVersion`.
CHART_YAML="charts/mikebom-operator/Chart.yaml"
CHART_VERSION="$(yq -r '.version' "$CHART_YAML")"
CHART_APP_VERSION="$(yq -r '.appVersion' "$CHART_YAML")"

# Verify all four strings agree.
FAIL=0
for pair in \
  "Cargo.toml [workspace.package] version:$CARGO_VERSION" \
  "Chart.yaml version:$CHART_VERSION" \
  "Chart.yaml appVersion:$CHART_APP_VERSION"; do
  name="${pair%%:*}"
  value="${pair##*:}"
  if [ "$value" != "$EXPECTED" ]; then
    echo "ERROR: $name = '$value' does not match git tag (stripped) '$EXPECTED'" >&2
    FAIL=1
  fi
done

if [ "$FAIL" -ne 0 ]; then
  echo "" >&2
  echo "Tag:                      $TAG (stripped: $EXPECTED)" >&2
  echo "Cargo.toml:               $CARGO_VERSION" >&2
  echo "Chart.yaml version:       $CHART_VERSION" >&2
  echo "Chart.yaml appVersion:    $CHART_APP_VERSION" >&2
  echo "" >&2
  echo "Fix the drifted file(s), commit, then re-tag (or force-push the tag)." >&2
  exit 1
fi

echo "✓ Version check passed: all four strings agree on '$EXPECTED'"
