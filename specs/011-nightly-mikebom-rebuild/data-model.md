# Phase 1 Data Model: Nightly mikebom rebuild

This feature has no runtime data model (it's release infra), but it does
have durable entities in the workflow graph — inputs, intermediate state,
and outputs. Documenting them here anchors the tests, contracts, and
runbook.

## Entities

### Mikebom Alpha Release

Represents a published `v0.1.0-alpha.N` release of `kusari-oss/mikebom`.

| Field | Type | Source | Notes |
|-------|------|--------|-------|
| `tag_name` | string | `gh api /repos/kusari-oss/mikebom/releases` | Must match regex `^v0\.1\.0-alpha\.[0-9]+$` for the workflow to consider it. |
| `alpha_num` | integer | derived from `tag_name` via `sed 's/.*alpha\.//'` | Used for semver-monotonic comparison. |
| `image_manifest_present` | boolean | `docker manifest inspect ghcr.io/kusari-oss/mikebom:<tag_name>` | FR-004 gate. If false, the release is not eligible. |
| `image_is_multiarch` | boolean | jq filter on manifest for `mediaType == "index"` AND both amd64 + arm64 entries | Enforces the operator's multi-arch guarantee. |
| `release_body_snippet` | string | `.body` from the release API | First ~2000 chars quoted in the bump PR body for maintainer context. |

**Uniqueness**: `tag_name` is unique per mikebom release. `alpha_num` is
unique among alphas (mikebom never re-uses a number).

**Ordering**: alphas are strictly monotonically increasing by `alpha_num`.
The workflow selects the highest `alpha_num` whose `image_manifest_present`
AND `image_is_multiarch` are both true AND whose `tag_name` is not in the
known-bad set (below).

---

### Operator Alpha Release

Represents a published `v0.1.0-alpha.N` git tag on `kusari-oss/mikebom-operator`.

| Field | Type | Source | Notes |
|-------|------|--------|-------|
| `tag_name` | string | `git tag -l 'v0.1.0-alpha.*'` | Must match regex `^v0\.1\.0-alpha\.[0-9]+$`. |
| `alpha_num` | integer | derived | Monotonic increment. |
| `pinned_mikebom_tag` | string | reads from Chart.yaml `mikebom.image` at the tag's tree | Which mikebom alpha the operator was pinning at that tag. |

**Ordering**: strictly monotonic. The bump script computes the next
operator tag as `v0.1.0-alpha.<max(alpha_num)+1>`.

**Bootstrap edge case**: if no matching tag exists (fresh repo), the
nightly no-ops with the diagnostic "no prior operator alpha tag; refusing
to guess initial version" (per FR spec edge case bullet).

---

### Nightly Bump PR

Represents an open or recently-closed PR authored by the nightly bot on
`kusari-oss/mikebom-operator`.

| Field | Type | Source | Notes |
|-------|------|--------|-------|
| `number` | integer | `gh pr create` return / `gh pr list` | Used for `gh pr merge --auto`. |
| `title` | string | schema `chore(nightly): bump mikebom to <mikebom_tag> and operator to <operator_tag>` | Machine-parseable; used to derive the target mikebom tag from closed PRs (for the known-bad set). |
| `body` | markdown | generated | Summary, verification results, quoted mikebom release notes. See contracts/nightly-workflow.md "PR body schema". |
| `head_branch` | string | schema `automation/nightly-bump/<mikebom_tag>` | Branch prefix identifies bot PRs even without a label. |
| `label` | string | fixed value `nightly-mikebom-bump` | Applied on `gh pr create`; used by `gh pr list --label` in the known-bad derivation. |
| `state` | enum { open, closed_merged, closed_unmerged } | `gh pr list --json state,mergedAt` | Drives the known-bad derivation and FR-018 stale-PR check. |
| `commit_trailer` | string | `Nightly-Bump-Target: <mikebom_tag>` | Appended to the bump commit's message; used by `tag-on-nightly-merge.yml` to detect the merged bump. |
| `has_cleared_label` | boolean | `nightly-mikebom-bump/cleared` label present | Maintainer-set override that removes a closed PR from the known-bad derivation. |

**State transitions**:

```
              (nightly run)                 (auto-merge on green CI)
[nonexistent] ─────────────► [open] ─────────────────────────────► [closed_merged]
                                │                                          │
                                │ (CI failed / maintainer close)           │ (tag-on-nightly-merge.yml fires)
                                ▼                                          ▼
                     [closed_unmerged]                              [operator tag pushed]
                                │                                          │
                                │ (maintainer re-opens                     │ (release.yml fires)
                                │  OR adds /cleared label)                 ▼
                                ▼                                    [release published]
                     [known-bad cleared]
```

---

### Known-Bad Set (derived)

Not a stored entity — computed on demand.

**Computation**:
```
gh pr list \
  --state closed \
  --label nightly-mikebom-bump \
  --json title,mergedAt,labels \
  --jq '[.[] | select(.mergedAt == null and (.labels | map(.name) | index("nightly-mikebom-bump/cleared") | not))
         | .title
         | capture("bump mikebom to (?<t>v0\\.1\\.0-alpha\\.[0-9]+)")
         | .t]'
```

Returns an array of mikebom tag names that failed. The detection step
checks whether the selected mikebom tag is in this array; if so, the run
no-ops with the reason "skipping <tag>: prior nightly PR closed unmerged".

**Bounded lookup**: `gh pr list` defaults to 30 PRs; explicit `--limit 100`
ensures we cover ~3 months of bumps at daily cadence. Older PRs are
irrelevant because mikebom has moved past them.

---

### Nightly Run

Represents one execution of `nightly-mikebom-bump.yml`.

| Field | Type | Source | Notes |
|-------|------|--------|-------|
| `run_id` | integer | `GITHUB_RUN_ID` | For the failure-issue body + PR body. |
| `run_url` | string | `${GITHUB_SERVER_URL}/${GITHUB_REPOSITORY}/actions/runs/${GITHUB_RUN_ID}` | Same. |
| `event` | enum { schedule, workflow_dispatch } | `github.event_name` | Whether cron-fired or manual. |
| `dry_run` | boolean | `inputs.dry_run` if workflow_dispatch, else false | Enables the dry-run branch (no push, no PR). |
| `decision` | enum { noop_up_to_date, noop_open_pr_exists, noop_known_bad, noop_no_operator_baseline, bump_opened_pr } | derived at end | Written into `$GITHUB_STEP_SUMMARY`. |
| `failure_signature` | string \| null | derived on failure | Format `<step_name>:<exit_code_class>`. Used for de-dup in FR-016. |

**Summary line format** (written to the workflow's run summary via
`$GITHUB_STEP_SUMMARY`):

```
| Field | Value |
|-------|-------|
| Decision | bump_opened_pr |
| Current pin | v0.1.0-alpha.57 |
| Latest mikebom | v0.1.0-alpha.58 |
| Known-bad | [] |
| PR opened | #200 |
| Next operator tag | v0.1.0-alpha.2 |
```

---

### Failure Issue (FR-012 + FR-016)

Represents a maintainer-facing GitHub issue filed when the nightly fails.

| Field | Type | Source | Notes |
|-------|------|--------|-------|
| `title` | string | schema `[nightly] failed at step <step_name>` | Deterministic; used for de-dup search. |
| `body` | markdown | generated | Contains run URL, step name, exit code, repo state (clean/partial), traceback if available. |
| `label` | string | fixed value `nightly-mikebom-bump/failure` | For filtering + notification routing. |
| `dedup_search` | issue query | `gh issue list --label nightly-mikebom-bump/failure --state open --search "step_name:<step>"` | Before filing, look for an open issue with the same step; if found, comment instead. |

---

## Requirement → Artifact Mapping

Every FR is grounded in one or more entities and artifacts.

| FR | Entities touched | Artifact / file |
|----|-----------------|-----------------|
| FR-001 | Nightly Run | `nightly-mikebom-bump.yml`'s `schedule` + `workflow_dispatch` triggers |
| FR-002 | Operator Alpha Release, Nightly Run | `nightly-detect.sh` reads `charts/mikebom-operator/values.yaml` via `yq` |
| FR-003 | Mikebom Alpha Release | `nightly-detect.sh` calls `gh api /repos/kusari-oss/mikebom/releases` |
| FR-004 | Mikebom Alpha Release | `nightly-detect.sh` calls `docker manifest inspect` |
| FR-005 | Nightly Bump PR | `nightly-bump.sh` (grep-based file discovery + sed) |
| FR-006 | Nightly Run | `nightly-detect.sh` exits with `decision=noop_up_to_date` |
| FR-007 | Operator Alpha Release | `nightly-bump.sh` reads `git tag -l 'v0.1.0-alpha.*'` |
| FR-008 | Operator Alpha Release, Nightly Bump PR | `nightly-bump.sh` writes Cargo.toml + Chart.yaml in one commit |
| FR-009 | Nightly Bump PR | `ci.yml` runs on the auto-opened PR (existing infra) |
| FR-010 | Nightly Run | `concurrency:` block on both new workflows |
| FR-011 | Mikebom Alpha Release, Operator Alpha Release | `nightly-detect.sh` semver comparison |
| FR-012 | Failure Issue | `nightly-mikebom-bump.yml`'s `on: failure` step calls `gh issue create` |
| FR-013 | Nightly Run | `workflow_dispatch` trigger with `dry_run: false` default in that path |
| FR-014 | Operator Alpha Release | `tag-on-nightly-merge.yml` pushes tag on `main` after squash-merge |
| FR-015 | Nightly Bump PR, Operator Alpha Release | `nightly-open-pr.sh` calls `gh pr merge --auto`; `tag-on-nightly-merge.yml` fires on merge |
| FR-016 | Failure Issue | `nightly-mikebom-bump.yml`'s failure step does `gh issue list ... --search ...` before create |
| FR-017 | Known-Bad Set (derived) | `nightly-detect.sh` runs the `gh pr list` query from R3 |
| FR-018 | Nightly Bump PR | `nightly-detect.sh` runs `gh pr list --state open --label nightly-mikebom-bump` |
