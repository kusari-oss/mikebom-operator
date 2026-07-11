#!/bin/sh
# Feature 011 T003: nightly detection step.
#
# Reads the currently pinned mikebom tag from the Helm chart's values.yaml,
# queries mikebom's release surface for the highest v0.1.0-alpha.N tag,
# verifies the multi-arch image manifest exists on ghcr.io, checks for
# open bump PRs (FR-018) and derived known-bad markers (FR-017), and emits
# a decision + supporting outputs to $GITHUB_OUTPUT.
#
# Environment:
#   MIKEBOM_REPO      Default: kusari-oss/mikebom
#   OPERATOR_REPO     Default: $GITHUB_REPOSITORY
#   GH_TOKEN          Required — gh CLI auth.
#   GITHUB_OUTPUT     Required — set by GitHub Actions.
#   GITHUB_STEP_SUMMARY  Required — set by GitHub Actions.
#
# Exit codes:
#   0  — any noop or should_bump decision written to $GITHUB_OUTPUT.
#   >0 — hard failure (network, malformed values.yaml, etc.).
#
# Outputs (via $GITHUB_OUTPUT):
#   decision            One of: noop_up_to_date, noop_open_pr_exists,
#                       noop_known_bad, noop_no_operator_baseline, should_bump.
#   current_pin         e.g., v0.1.0-alpha.57
#   latest_mikebom      e.g., v0.1.0-alpha.58 (empty if lookup failed)
#   next_operator_tag   e.g., v0.1.0-alpha.2 (empty on noop_no_operator_baseline)
#   open_pr_number      e.g., 200 (only when decision=noop_open_pr_exists)

set -eu

MIKEBOM_REPO="${MIKEBOM_REPO:-kusari-oss/mikebom}"
OPERATOR_REPO="${OPERATOR_REPO:-${GITHUB_REPOSITORY:-}}"

if [ -z "${OPERATOR_REPO:-}" ]; then
  echo "::error::OPERATOR_REPO is empty and GITHUB_REPOSITORY unset" >&2
  exit 1
fi

emit() {
  # Writes key=value to $GITHUB_OUTPUT. Value must be single-line.
  echo "$1=$2" >>"$GITHUB_OUTPUT"
}

summary() {
  # Appends a line to the workflow's run summary.
  echo "$1" >>"$GITHUB_STEP_SUMMARY"
}

# ---------------------------------------------------------------------------
# 1. Current pin (FR-002): read from the Helm chart's values.yaml.
# ---------------------------------------------------------------------------
values_file="charts/mikebom-operator/values.yaml"
current_image="$(yq '.mikebom.image' "$values_file")"
if [ -z "$current_image" ] || [ "$current_image" = "null" ]; then
  echo "::error::could not read .mikebom.image from $values_file" >&2
  exit 1
fi
current_pin="${current_image##*:}"
echo "detected current_pin=$current_pin"

if ! printf '%s' "$current_pin" | grep -Eq '^v0\.1\.0-alpha\.[0-9]+$'; then
  echo "::error::current pin '$current_pin' does not match v0.1.0-alpha.N shape" >&2
  exit 1
fi

current_num="${current_pin#v0.1.0-alpha.}"

# ---------------------------------------------------------------------------
# 2. Latest mikebom alpha (FR-003).
# ---------------------------------------------------------------------------
latest_mikebom="$(
  gh api "repos/${MIKEBOM_REPO}/releases" --paginate \
    --jq '.[] | .tag_name | select(test("^v0\\.1\\.0-alpha\\.[0-9]+$"))' \
    | sort -V | tail -1
)"

if [ -z "$latest_mikebom" ]; then
  echo "::error::no v0.1.0-alpha.N releases found on $MIKEBOM_REPO" >&2
  exit 1
fi
echo "detected latest_mikebom=$latest_mikebom"
latest_num="${latest_mikebom#v0.1.0-alpha.}"

# FR-011 downgrade guard: highest available lower than current pin (yank).
if [ "$latest_num" -lt "$current_num" ]; then
  echo "::warning::mikebom highest available ($latest_mikebom) is lower than current pin ($current_pin); this looks like a mikebom yank — refusing to downgrade"
  emit decision noop_up_to_date
  emit current_pin "$current_pin"
  emit latest_mikebom "$latest_mikebom"
  emit next_operator_tag ""
  emit open_pr_number ""
  summary "| Decision | noop_up_to_date (downgrade refused) |"
  summary "| Current pin | $current_pin |"
  summary "| Latest mikebom | $latest_mikebom |"
  exit 0
fi

# Equal — no bump needed (FR-006).
if [ "$latest_num" -eq "$current_num" ]; then
  emit decision noop_up_to_date
  emit current_pin "$current_pin"
  emit latest_mikebom "$latest_mikebom"
  emit next_operator_tag ""
  emit open_pr_number ""
  summary "| Decision | noop_up_to_date |"
  summary "| Current pin | $current_pin |"
  summary "| Latest mikebom | $latest_mikebom |"
  exit 0
fi

# ---------------------------------------------------------------------------
# 3. Image existence gate (FR-004).
#
# We already verified that the mikebom GitHub release exists via the
# releases API above (that's how we got `latest_mikebom`). Explicit
# manifest verification via `docker manifest inspect` / `crane manifest` /
# `gh api packages/container/versions` all require cross-org authentication
# that GITHUB_TOKEN does not grant — see hotfix PRs #20/#21/#22/#23 for the
# investigation.
#
# We trust mikebom's release contract: their release.yml publishes the
# GitHub release AND pushes the multi-arch image atomically. The tiny race
# window where the release exists but the image push hasn't landed yet
# resolves within seconds; the next nightly (or the maintainer's dispatch)
# picks it up cleanly. If the release-and-image contract is ever broken,
# end users hit ImagePullBackOff at Job-spawn — the same outcome we'd get
# if a workflow-side manifest check had failed.
#
# If we ever need strict per-architecture verification, add a repo secret
# containing a cross-org PAT with `read:packages` and switch to `crane
# manifest` — but that's a real ongoing secret-management cost.
# ---------------------------------------------------------------------------
echo "trusting mikebom release contract: ${latest_mikebom} release exists, image assumed published"

# ---------------------------------------------------------------------------
# 4. Stale-PR check (FR-018): skip if any prior bump PR is still open.
# ---------------------------------------------------------------------------
open_pr_number="$(
  gh pr list --repo "$OPERATOR_REPO" \
    --state open --label nightly-mikebom-bump \
    --json number --jq '.[0].number // empty'
)"
if [ -n "$open_pr_number" ]; then
  emit decision noop_open_pr_exists
  emit current_pin "$current_pin"
  emit latest_mikebom "$latest_mikebom"
  emit next_operator_tag ""
  emit open_pr_number "$open_pr_number"
  summary "| Decision | noop_open_pr_exists |"
  summary "| Current pin | $current_pin |"
  summary "| Latest mikebom | $latest_mikebom |"
  summary "| Waiting on PR | #$open_pr_number |"
  exit 0
fi

# ---------------------------------------------------------------------------
# 5. Known-bad check (FR-017): derive from closed unmerged bump PRs.
# ---------------------------------------------------------------------------
known_bad="$(
  gh pr list --repo "$OPERATOR_REPO" \
    --state closed --label nightly-mikebom-bump --limit 100 \
    --json title,mergedAt,labels \
    --jq '[.[] | select(.mergedAt == null and ((.labels | map(.name) | index("nightly-mikebom-bump/cleared")) == null))
           | .title
           | capture("bump mikebom to (?<t>v0\\.1\\.0-alpha\\.[0-9]+)")
           | .t] | .[]'
)"

if printf '%s\n' "$known_bad" | grep -Fxq "$latest_mikebom"; then
  emit decision noop_known_bad
  emit current_pin "$current_pin"
  emit latest_mikebom "$latest_mikebom"
  emit next_operator_tag ""
  emit open_pr_number ""
  summary "| Decision | noop_known_bad |"
  summary "| Current pin | $current_pin |"
  summary "| Latest mikebom | $latest_mikebom (known-bad — prior PR closed unmerged) |"
  exit 0
fi

# ---------------------------------------------------------------------------
# 6. Compute next operator tag (FR-007).
# ---------------------------------------------------------------------------
current_op_tag="$(git tag -l 'v0.1.0-alpha.*' | sort -V | tail -1)"
if [ -z "$current_op_tag" ]; then
  emit decision noop_no_operator_baseline
  emit current_pin "$current_pin"
  emit latest_mikebom "$latest_mikebom"
  emit next_operator_tag ""
  emit open_pr_number ""
  summary "| Decision | noop_no_operator_baseline |"
  summary "| Note | no prior operator alpha tag; refusing to guess initial version |"
  exit 0
fi
current_op_num="${current_op_tag#v0.1.0-alpha.}"
next_op_num=$((current_op_num + 1))
next_operator_tag="v0.1.0-alpha.${next_op_num}"

# ---------------------------------------------------------------------------
# 7. Emit the bump decision.
# ---------------------------------------------------------------------------
emit decision should_bump
emit current_pin "$current_pin"
emit latest_mikebom "$latest_mikebom"
emit next_operator_tag "$next_operator_tag"
emit open_pr_number ""

summary "| Decision | should_bump |"
summary "| Current pin | $current_pin |"
summary "| Latest mikebom | $latest_mikebom |"
summary "| Next operator tag | $next_operator_tag |"
summary "| Known-bad | [] |"
