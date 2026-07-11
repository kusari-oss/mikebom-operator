# Contract: `.github/workflows/tag-on-nightly-merge.yml`

Machine-checkable interface for the post-merge tag-push workflow that
completes the hybrid PR-then-tag delivery (FR-015).

## Trigger

```yaml
on:
  push:
    branches: [main]
```

No path filter — the workflow's first step is a HEAD-commit inspection
that gates cheap.

## Permissions (workflow-level)

```yaml
permissions:
  contents: write   # Push tag
```

No PR / issue writes needed. No `id-token: write`.

## Concurrency

```yaml
concurrency:
  group: tag-on-nightly-merge
  cancel-in-progress: false
```

Serializes tag pushes so that back-to-back merges (unusual but possible if
a maintainer rebases a fix onto a bump commit) don't race.

## Step-by-step contract

### 1. `checkout`
- **Action**: `actions/checkout` (SHA pinned, reused).
- **`fetch-depth: 0`** and **`fetch-tags: true`** — needed to inspect
  history and to read existing tags for the idempotency check.

### 2. `detect_nightly_bump`
- **Runs**: inline
  ```bash
  trailer=$(git log -1 --format=%B | grep -m1 '^Nightly-Bump-Target:' || true)
  if [ -z "$trailer" ]; then
    echo "not-a-nightly-bump=true" >> "$GITHUB_OUTPUT"
    echo "::notice::HEAD is not a nightly bump commit; nothing to tag"
    exit 0
  fi
  target=$(echo "$trailer" | sed -E 's/^Nightly-Bump-Target:[[:space:]]*//')
  echo "mikebom-target=$target" >> "$GITHUB_OUTPUT"
  ```
- **Behavior**: exits successfully with `not-a-nightly-bump=true` output
  for every push that isn't a nightly bump. This makes the workflow safe
  to run on every push to `main`.

### 3. `resolve_operator_tag` (conditional)
- **`if`**: `steps.detect_nightly_bump.outputs.not-a-nightly-bump != 'true'`
- **Runs**: inline
  ```bash
  app_version=$(yq '.appVersion' charts/mikebom-operator/Chart.yaml)
  operator_tag="v${app_version}"
  echo "operator-tag=$operator_tag" >> "$GITHUB_OUTPUT"
  ```
- **Env**: `yq` from `mikefarah/yq` action (SHA pinned).

### 4. `check_tag_idempotency`
- **`if`**: same conditional.
- **Runs**: inline
  ```bash
  if git rev-parse "refs/tags/${operator_tag}" >/dev/null 2>&1; then
    echo "already-tagged=true" >> "$GITHUB_OUTPUT"
    echo "::warning::${operator_tag} already exists; skipping tag push"
    exit 0
  fi
  ```

### 5. `push_tag`
- **`if`**: `not-a-nightly-bump != 'true' && already-tagged != 'true'`
- **Runs**: inline
  ```bash
  git config user.name "github-actions[bot]"
  git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
  git tag -a "${operator_tag}" -m "Nightly release ${operator_tag}"
  git push origin "refs/tags/${operator_tag}"
  echo "::notice::Pushed tag ${operator_tag}; release.yml will fire"
  ```
- **Post-condition**: the pushed tag triggers `release.yml` via its `on:
  push: tags: - v*` trigger. This workflow's responsibility ends here.

## Idempotency and safety

- **Safe on every push**: step 2 short-circuits non-bump commits with
  zero cost.
- **Safe on re-runs**: step 4 refuses to push a tag that already exists.
- **Safe against tag collisions with manual releases**: `release.yml`'s
  existing `versions` job would fail if a maintainer manually tagged
  ahead of the nightly (the tag's Chart.yaml wouldn't match the tag). We
  never overwrite an existing tag.

## Failure semantics

| Failure | Behavior |
|---------|----------|
| Not a nightly bump commit | Workflow succeeds (no-op branch) |
| Tag already exists | Workflow succeeds with warning annotation (no re-push) |
| `git push origin refs/tags/…` rejected (branch protection on tags) | Workflow fails; no signal — this is a repo-config bug that should surface via GitHub's default failure notification |
| Trailer parse produces an invalid semver | Workflow fails; the malformed trailer is a bug in `nightly-open-pr.sh`, must be fixed in-code |

## Non-goals

- Does NOT open PRs or file issues.
- Does NOT verify mikebom-alpha compatibility (that already happened on
  the nightly bump PR's CI).
- Does NOT sign the tag. `release.yml`'s cosign flow signs the image and
  chart artifacts; the git tag itself is unsigned by convention (same as
  today's manual releases per feature 010).
