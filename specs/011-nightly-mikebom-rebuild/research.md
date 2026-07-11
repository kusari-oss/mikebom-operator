# Phase 0 Research: Nightly mikebom rebuild

Seven decision points collected from the plan's Technical Context. Each entry
records the decision, the rationale, and the discarded alternatives.

---

## R1. Bot identity for opening PRs + enabling auto-merge

**Decision**: Use the default workflow `GITHUB_TOKEN` with elevated
per-workflow permissions (`contents: write`, `pull-requests: write`,
`issues: write`). Configure the repo's branch protection on `main` to
"Allow auto-merge" and to permit merges via `GITHUB_TOKEN`. Do NOT introduce
a dedicated GitHub App or long-lived PAT for the v1 of this feature.

**Rationale**:
- Zero secret sprawl. `GITHUB_TOKEN` is minted per-run, scoped to the repo,
  and expires when the workflow ends. No storage, no rotation, no PAT
  quarterly review.
- Sufficient for auto-merge in most repos with branch protection: GitHub's
  auto-merge behavior with `GITHUB_TOKEN` respects branch protection rules
  including required checks (Kusari Inspector counts here).
- The signed release is still cut by `release.yml` on tag push, which uses
  keyless OIDC cosign (already validated in feature 010). The nightly bot's
  identity never signs anything.

**Alternatives considered**:
- **Dedicated GitHub App** (e.g., `kusari-nightly-bot[bot]`): cleaner identity
  in PR history but adds an installation + private key stored as a secret.
  Defer to v2 if `GITHUB_TOKEN` proves insufficient.
- **Fine-grained PAT** stored as a repo secret: fragile (owner rotation
  breaks it), high blast-radius if leaked. Rejected.
- **Deploy key**: cannot open PRs. Not applicable.

**Risk mitigation**: if `GITHUB_TOKEN` cannot auto-merge on `main` in
practice (because `main`'s branch protection requires "review by CODEOWNERS"
or similar), the fallback is to fail closed — the PR sits open, the failure
signal is filed, maintainer sees it. Not silent, not broken, just not
autonomous. The runbook covers the escalation.

---

## R2. Auto-merge mechanism

**Decision**: `gh pr merge <number> --auto --squash` invoked immediately
after `gh pr create`. Squash merge produces a single clean bump commit on
`main`, which is exactly what `tag-on-nightly-merge.yml` needs to detect
via commit-trailer.

**Rationale**:
- `gh pr merge --auto` is GitHub's native auto-merge feature — the PR sits
  until all required checks pass, then merges automatically. No polling
  needed from our workflow, no secondary "watcher" workflow.
- Squash merge produces one merge commit on `main` per bump, which
  simplifies the tag-push detection: the commit trailer we add during
  PR creation survives squash-merge into the final commit message.
- `--squash` also keeps `main`'s history clean of the bot's local commit
  ordering (any fix-up commits the maintainer might push to the PR get
  squashed away).

**Alternatives considered**:
- **Rebase merge**: preserves commit history but complicates the
  trailer-detection heuristic (multiple commits, only one carries the
  trailer). Rejected.
- **Merge commit**: adds a merge commit above the bump commit, which
  breaks the "read HEAD trailer" detection unless we walk history.
  Rejected.
- **GitHub GraphQL `enablePullRequestAutoMerge` mutation** (raw API): more
  flexible but adds a JSON payload and error-handling burden. `gh pr merge
  --auto` is a thin wrapper over this same API. Prefer the wrapper.
- **Custom watcher workflow that polls PR status and merges when green**:
  reinvents auto-merge. Rejected.

---

## R3. Known-bad set (FR-017) — state file vs. PR-history-derived

**Decision**: PR-history-derived. On each nightly run, query recently-closed
bump PRs authored by the bot with the `nightly-mikebom-bump` label, filter
to those that are closed-unmerged, extract the target mikebom tag from each
PR's title (which follows a fixed schema — see contracts/nightly-workflow.md
"PR Schema" section), and treat that set as known-bad. No state file
committed to the repo.

**Rationale**:
- No chicken-and-egg problem. A state file approach requires the nightly to
  commit "alpha.58 failed" AFTER the alpha.58 PR itself failed — but the
  failing PR is the only way for the bot to modify the repo. That would
  require a second follow-up PR just to record the failure, which is
  convoluted.
- PR history is already durable and auditable. `gh pr list --state closed
  --author @me --label nightly-mikebom-bump --json` returns the exact data
  we need with no additional infrastructure.
- Maintainer override is natural: if the maintainer wants to unmark alpha.58
  as known-bad, they either re-open the PR (and gh pr list stops seeing it
  as closed-unmerged) or add a `nightly-mikebom-bump/cleared` label (the
  detection script filters that label out).
- Aligns with FR-017's "durable in-repo record" — GitHub's PR history is
  a durable, in-repo (repo-scoped) record.

**Alternatives considered**:
- **State file** (`.github/nightly-state.json`): the naive read of FR-017.
  Rejected because of the chicken-and-egg problem above — the file update
  can only happen via a PR, which itself might fail CI, requiring a
  follow-up "state-file-only" PR. Complex.
- **Repo variable / GitHub Actions variable**: not durable across workflow
  runs in the intended sense (variables are mutable but modification
  requires elevated permissions). Rejected.
- **Git note** on the failing commit: obscure, poor discoverability for
  maintainers, hard to inspect via `gh` CLI. Rejected.

**Recovery path**: to clear a known-bad marker, the maintainer either
(a) reopens the closed unmerged bump PR (workflow re-tries alpha.N on
next run because it's no longer "closed"), or (b) adds the
`nightly-mikebom-bump/cleared` label to the closed PR (script filters it
out). Both are documented in the runbook.

---

## R4. Post-merge tag-push trigger

**Decision**: A dedicated `.github/workflows/tag-on-nightly-merge.yml`
triggered on `push` to `main`, filtered by inspecting the last commit's
trailer for `Nightly-Bump-Target: v0.1.0-alpha.N`. When the trailer is
present, the workflow reads Chart.yaml's `appVersion`, pushes the
corresponding `v<appVersion>` tag using the same `GITHUB_TOKEN`, and exits.
The existing `release.yml` fires on the tag push and does everything else
(build, sign, attest, publish).

**Rationale**:
- Clean separation: the nightly workflow deals with detection + PR + auto-
  merge; the tag workflow deals with the post-merge tag push. Neither
  workflow needs to poll or wait.
- Commit-trailer detection is robust and self-documenting: `git log -1
  --format=%B | grep -q '^Nightly-Bump-Target:'`. Doesn't rely on commit
  author (which is `github-actions[bot]` for many merges, not just ours).
- The tag push triggers `release.yml` unchanged. Zero delta on the
  release pipeline itself.

**Alternatives considered**:
- **`workflow_run` trigger** on the nightly workflow's completion: not
  usable for our case because we auto-merge asynchronously; the nightly
  workflow completes long before the PR merges. The `workflow_run` fires
  on the WORKFLOW's end, not on downstream events. Rejected.
- **Include the tag push inside the nightly workflow after gh pr merge
  --auto**: would require the workflow to poll until auto-merge completes,
  which could take 30+ minutes. Wasteful. Rejected.
- **Manual tag push by maintainer after each merge**: defeats "nightly
  release" framing. Rejected per the specify-phase clarification.
- **Repository dispatch webhook from GitHub App**: no GitHub App in v1 (see
  R1). Deferred.

---

## R5. Multi-arch manifest verification method

**Decision**: `docker manifest inspect ghcr.io/kusari-oss/mikebom:v0.1.0-alpha.N`
via the runner's preinstalled Docker CLI, plus a `jq` filter that asserts
the response has `mediaType == "application/vnd.oci.image.index.v1+json"`
AND at least one entry with `platform.architecture == "amd64"` AND at least
one with `arm64`.

**Rationale**:
- `docker manifest inspect` is already how the maintainer verified the
  bump manually earlier this session (see the conversation history — same
  command validated `v0.1.0-alpha.57` before the manual bump).
- No auth needed for the public `ghcr.io/kusari-oss/mikebom` namespace.
- `ubuntu-latest` runners come with Docker preinstalled.

**Alternatives considered**:
- **`crane manifest ghcr.io/kusari-oss/mikebom:v0.1.0-alpha.N`**: requires
  installing `crane` (an extra step, a SHA-pinned action or download).
  Marginally cleaner output but not worth the extra dependency.
- **Raw HTTPS GET on `https://ghcr.io/v2/kusari-oss/mikebom/manifests/vX`**:
  requires bearer token acquisition; more moving parts. Rejected for the
  public-image case.
- **Skip verification, trust the GitHub Release**: violates FR-004
  ("verify that a multi-arch image manifest exists"). Rejected.

---

## R6. Latest-alpha lookup source: GitHub API vs. registry API

**Decision**: `gh api repos/kusari-oss/mikebom/releases --paginate --jq
'.[] | .tag_name | select(test("^v0\\.1\\.0-alpha\\.[0-9]+$"))'` — filter
to alpha-shaped tags, semver-sort, pick the highest.

**Rationale**:
- Same auth as the rest of the workflow (`GITHUB_TOKEN`).
- GitHub's release list is authoritative for what mikebom's maintainers
  intended to publish; a container image that exists on GHCR without a
  corresponding release is not a valid signal for the nightly (could be
  an accidental push).
- The GitHub API also gives us the release notes body, which we quote
  into the bump PR's description for maintainer context. That's free with
  this approach; the registry approach would require a separate lookup.

**Alternatives considered**:
- **`crane ls ghcr.io/kusari-oss/mikebom`** to enumerate tags directly:
  no dependency on mikebom repo access (would work even if the mikebom
  repo were private). Rejected in v1 because kusari-oss/mikebom is public
  and the release list is richer.
- **Query both** (release list + registry ls), require both to agree:
  overkill for v1. Belt-and-suspenders paranoia. Deferred.
- **RSS/atom feed on releases**: available but not richer than the API.
  Rejected.

**Rate-limit note**: `GITHUB_TOKEN` gives 1000 req/hr per repo. Nightly
uses ~3-5 requests per run. Trivially within budget.

---

## R7. Concurrency lock

**Decision**: Both new workflows declare a `concurrency:` block at the
workflow level:
- `nightly-mikebom-bump.yml`: `group: nightly-mikebom-bump` +
  `cancel-in-progress: false`. Ensures only one nightly detection runs at
  a time (schedule + workflow_dispatch can otherwise collide if a
  maintainer clicks Run while the cron fires).
- `tag-on-nightly-merge.yml`: `group: tag-on-nightly-merge` +
  `cancel-in-progress: false`. Ensures serialized tag pushes.

The nightly workflow does NOT lock against `release.yml` — the two operate
on disjoint git refs (nightly touches a bot-authored PR branch and
main; `release.yml` touches tags). Cross-workflow interference is
prevented by FR-018's pre-run PR check, not by a shared lock.

**Rationale**:
- GitHub Actions `concurrency:` is the platform-native primitive; no
  external lock service needed.
- `cancel-in-progress: false` means a queued run waits for the current
  one instead of aborting it — important because aborting mid-run could
  leave a PR half-opened.

**Alternatives considered**:
- **Cross-workflow lock** using a shared `concurrency` group that both
  nightly workflows AND `release.yml` share: overcautious. Rejected —
  `release.yml` runs on tag pushes, which is a different logical epoch
  from PR-opening.
- **In-repo lockfile**: bespoke, error-prone. Rejected.

---

## Summary

All 7 decisions resolved. No open NEEDS CLARIFICATION items remain from
the plan's Technical Context. Ready for Phase 1.
