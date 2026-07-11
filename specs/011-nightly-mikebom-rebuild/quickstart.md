# Quickstart: Nightly mikebom rebuild — maintainer's operational guide

Everything the maintainer needs to know to trust, verify, and operate the
nightly workflow. This becomes §5 of `docs/release-runbook.md` after
implementation.

## 1. What the nightly does, in one paragraph

At 03:17 UTC every day, `nightly-mikebom-bump.yml` fires. It reads the
current mikebom pin from `charts/mikebom-operator/values.yaml`, queries
the highest `v0.1.0-alpha.*` release on `kusari-oss/mikebom`, verifies
the multi-arch image manifest exists on ghcr.io, checks that no prior
bump PR is still open (skip if so — FR-018) and that the target alpha
isn't in the derived known-bad set (skip if so — FR-017). If all checks
pass and the target alpha is strictly newer than the current pin, the
workflow bumps every live reference in the repo (same 12-file surface
as the manual `v0.1.0-alpha.57` bump), regenerates the CRD via
`mikebom-operator-ctl`, bumps the operator's own version in structural
lockstep across Cargo.toml + Chart.yaml, opens a PR, and enables
`gh pr merge --auto --squash`. When CI + Kusari Inspector go green, the
PR auto-merges to `main`. `tag-on-nightly-merge.yml` detects the merged
commit's `Nightly-Bump-Target:` trailer, pushes the corresponding
operator tag, and hands off to `release.yml` — which produces the same
signed, SBOM-attested artifacts as a manual release.

## 2. First-run rehearsal (dry-run)

Before enabling the schedule for the first time, rehearse the detection +
bump path against the current state:

```sh
gh workflow run nightly-mikebom-bump.yml -f dry_run=true
gh run watch                                     # tail the run
```

Expected output (assuming mikebom is currently at a newer alpha than the
operator's pin):

```
::notice::current_pin=v0.1.0-alpha.57 latest_mikebom=v0.1.0-alpha.58 decision=should_bump
::notice::[dry_run] Would push branch automation/nightly-bump/v0.1.0-alpha.58
::notice::[dry_run] Would open PR titled: chore(nightly): bump mikebom to v0.1.0-alpha.58 and operator to v0.1.0-alpha.2
::notice::[dry_run] Would enable auto-merge
```

Zero commits, zero branches on origin, zero PRs opened. If any of the
above steps errors, fix the issue before enabling the cron schedule.

## 3. First real-run inspection checklist

After the first real (non-dry) run opens a PR:

- [ ] PR title matches schema `chore(nightly): bump mikebom to
      <mikebom_tag> and operator to <operator_tag>`
- [ ] PR body contains verification checkmarks and mikebom release notes
      excerpt
- [ ] PR is labeled `nightly-mikebom-bump`
- [ ] PR branch is named `automation/nightly-bump/<mikebom_tag>`
- [ ] Commit trailer `Nightly-Bump-Target: <mikebom_tag>` present in the
      HEAD commit message
- [ ] Diff touches ONLY the 12-file live surface + Cargo.toml +
      Chart.yaml + regenerated CRD — does NOT touch `specs/003-*` or
      `specs/004-*`
- [ ] `cargo test --workspace --lib` passes on the PR
- [ ] CI + Kusari Inspector are running (both required for auto-merge)
- [ ] After auto-merge, `tag-on-nightly-merge.yml` fires on the resulting
      push, resolves `operator_tag` from Chart.yaml, and pushes the tag
- [ ] `release.yml` fires on the tag push and produces signed
      image + signed chart + SBOM attestation + GitHub Release page
- [ ] `cosign verify … --certificate-identity <run_url>` succeeds
      against the new image and chart (per feature 010's verification
      commands)

## 4. Common operational actions

### 4.1 Hold the nightly (don't bump for a stretch)

Disable the workflow via the GitHub UI (Actions → nightly-mikebom-bump →
"…" → Disable workflow), or by dispatching with `dry_run=true` and
leaving the schedule disabled. Re-enable when ready. The workflow is
stateless; disabling doesn't leave orphan branches.

### 4.2 Clear a known-bad marker

If the nightly has marked `v0.1.0-alpha.58` as known-bad and mikebom
subsequently patched the image (unusual — normally mikebom releases
`.59` instead), clear the marker by either:

- **Reopen** the closed unmerged PR (`gh pr reopen <n>`). The known-bad
  derivation will stop matching it.
- **Label as cleared**: `gh pr edit <n> --add-label
  nightly-mikebom-bump/cleared`. The detection script filters this
  label out.

The next nightly run will retry.

### 4.3 Force a bump on-demand

Push mikebom just released `.59` and you don't want to wait for cron:

```sh
gh workflow run nightly-mikebom-bump.yml
```

Uses the same code path as the scheduled run (FR-013). `dry_run` defaults
to false for `workflow_dispatch` (unlike the pre-flight rehearsal above
which explicitly passes `-f dry_run=true`).

### 4.4 Cancel a bump PR mid-flight

If CI is going to fail and you want to stop the auto-merge early:

```sh
gh pr close <n>                          # closes without merging
```

The next nightly will treat this PR as known-bad for its mikebom target.
If that's not what you want, also add:

```sh
gh pr edit <n> --add-label nightly-mikebom-bump/cleared
```

Which clears the known-bad status.

### 4.5 Roll back a bad release

The nightly workflow does NOT provide a rollback path (out of scope). If
a released operator alpha turns out to be broken, use the existing
`release.yml` rollback procedure documented in the runbook — cut a new
alpha (`v0.1.0-alpha.<N+1>`) that reverts the mikebom pin.

## 5. Trust boundaries

- The nightly workflow can push commits to bot branches (via
  `GITHUB_TOKEN`), open PRs, apply labels, file/comment issues, enable
  auto-merge. It CANNOT push directly to `main` (branch protection).
- The tag workflow can push tags. It CANNOT overwrite existing tags
  (idempotency check).
- Neither workflow can sign anything. Signing remains cosign-keyless-
  OIDC via `release.yml` on tag push (feature 010).
- Neither workflow can bypass Kusari Inspector or CI. The auto-merge
  fires only when both are green; if either is red, the PR sits and the
  failure surfaces via the FR-012 signal chain.

## 6. Troubleshooting

| Symptom | Likely cause | Fix |
|--------|-------------|-----|
| No PR opened despite mikebom moving | Prior unresolved PR (FR-018) | `gh pr list --label nightly-mikebom-bump --state open` — resolve the existing one |
| PR opened but auto-merge not enabled | `GITHUB_TOKEN` can't auto-merge on protected branch | Enable "Allow auto-merge" in repo Settings → General; consider a GitHub App if it still fails |
| Auto-merge fires but tag never pushed | `Nightly-Bump-Target:` trailer missing from merged commit | Squash-merge preserves the trailer; check `nightly-open-pr.sh`'s commit message. Non-squash merges would also break this — verify auto-merge strategy is `--squash` |
| Duplicate failure issues piling up | De-dup search failing | Check `gh issue list --label nightly-mikebom-bump/failure` and the search filter in `signal_failure` step; may need to broaden the search term |
| Nightly no-ops when mikebom clearly moved | Known-bad set incorrectly matching current target | Run detection manually with debug: `bash .github/scripts/nightly-detect.sh 2>&1` — inspect the derived known-bad list |
| Tag workflow fires on non-bump merges | Trailer accidentally present in a manual commit | Trailer is scoped to `Nightly-Bump-Target:` — collisions extremely unlikely, but the workflow's tag-idempotency step (§4 of contract) prevents damage |
