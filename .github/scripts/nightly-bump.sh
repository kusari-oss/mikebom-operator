#!/bin/sh
# Feature 011 T004: nightly bump step.
#
# Replaces the mikebom tag string across the 12 "live surface" files (same
# set the manual v0.1.0-alpha.57 bump touched), regenerates the CRD YAML
# from the Rust struct via mikebom-operator-ctl, bumps the operator's own
# version in structural lockstep across Cargo.toml + Chart.yaml, then runs
# a local fmt/build/unit-test gate.
#
# Environment:
#   MIKEBOM_OLD    Current pinned mikebom tag, e.g., v0.1.0-alpha.57
#   MIKEBOM_NEW    Target mikebom tag, e.g., v0.1.0-alpha.58
#   OPERATOR_NEW   New operator version tag, e.g., v0.1.0-alpha.2
#
# Excludes: specs/003-scan-job-builder/**, specs/004-pvc-backend/** (frozen
# historical spec artifacts).
#
# Exit non-zero on any failure. Callers route failure to signal_failure.

set -eu

: "${MIKEBOM_OLD:?MIKEBOM_OLD is required}"
: "${MIKEBOM_NEW:?MIKEBOM_NEW is required}"
: "${OPERATOR_NEW:?OPERATOR_NEW is required}"

# Sanity: OPERATOR_NEW must be a valid v0.1.0-alpha.N tag.
if ! printf '%s' "$OPERATOR_NEW" | grep -Eq '^v0\.1\.0-alpha\.[0-9]+$'; then
  echo "::error::OPERATOR_NEW '$OPERATOR_NEW' does not match v0.1.0-alpha.N" >&2
  exit 1
fi
operator_version="${OPERATOR_NEW#v}"

# ---------------------------------------------------------------------------
# 1. mikebom tag replacement across the 12 live-surface files (FR-005).
# ---------------------------------------------------------------------------
files="
charts/mikebom-operator/values.yaml
charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml
crates/operator/src/crds/namespace_scan.rs
crates/operator/src/scan_job/mod.rs
docs/crd-reference.md
examples/namespacescan.yaml
e2e/tests/job_status_feedback.rs
e2e/tests/reconciler_failover.rs
e2e/tests/reconciler_skeleton.rs
e2e/tests/reconciler_spawns_job.rs
e2e/tests/scan_job_dryrun.rs
e2e/tests/schedule_honoring.rs
"

for f in $files; do
  if [ ! -f "$f" ]; then
    echo "::error::expected file $f is missing" >&2
    exit 1
  fi
  # BSD/GNU sed portability: use perl -pi -e for cross-platform in-place edit.
  perl -pi -e "s|\Q$MIKEBOM_OLD\E|$MIKEBOM_NEW|g" "$f"
done

# ---------------------------------------------------------------------------
# 2. Regenerate the CRD YAML from the Rust struct (Constitution VII).
# ---------------------------------------------------------------------------
cargo run --release --bin mikebom-operator-ctl -- crd \
  > charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml

# ---------------------------------------------------------------------------
# 3. Bump operator version in structural lockstep (FR-008).
#    - Cargo.toml [workspace.package].version
#    - charts/mikebom-operator/Chart.yaml .version
#    - charts/mikebom-operator/Chart.yaml .appVersion
# ---------------------------------------------------------------------------
# Cargo.toml: match the version line inside [workspace.package]. The workspace
# package block is unique in Cargo.toml; a targeted regex is safer than yq.
perl -0pi -e '
  s{(\[workspace\.package\][^\[]*?version\s*=\s*")[^"]+(")}
  {${1}'"$operator_version"'${2}}s
' Cargo.toml

# Chart.yaml: yq handles both fields cleanly.
yq -i ".version = \"$operator_version\"" charts/mikebom-operator/Chart.yaml
yq -i ".appVersion = \"$operator_version\"" charts/mikebom-operator/Chart.yaml

# ---------------------------------------------------------------------------
# 4. Local gate (FR-009): fmt + build + unit tests.
#    E2Es are NOT run here — they gate at PR-CI time via ci.yml.
# ---------------------------------------------------------------------------
cargo fmt --all -- --check
cargo build --workspace
cargo test --workspace --lib --tests
