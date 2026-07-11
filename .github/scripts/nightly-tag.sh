#!/bin/sh
# Feature 011 T006: post-merge tag-push step.
#
# Called by tag-on-nightly-merge.yml after a squashed bump commit lands on
# main. Reads the operator tag from Chart.yaml's appVersion, checks tag
# idempotency, and pushes the tag — which triggers release.yml unchanged.
#
# Environment:
#   None required beyond a checked-out repo with fetch-tags: true.

set -eu

app_version="$(yq '.appVersion' charts/mikebom-operator/Chart.yaml)"
if [ -z "$app_version" ] || [ "$app_version" = "null" ]; then
  echo "::error::could not read .appVersion from Chart.yaml" >&2
  exit 1
fi
operator_tag="v${app_version}"

if git rev-parse "refs/tags/${operator_tag}" >/dev/null 2>&1; then
  echo "::warning::${operator_tag} already exists; skipping tag push (idempotent)"
  exit 0
fi

git config user.name "github-actions[bot]"
git config user.email "41898282+github-actions[bot]@users.noreply.github.com"

git tag -a "$operator_tag" -m "Nightly release ${operator_tag}"
git push origin "refs/tags/${operator_tag}"

echo "::notice::Pushed tag ${operator_tag}; release.yml will fire on the tag push."
