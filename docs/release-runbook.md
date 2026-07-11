# mikebom-operator release runbook

The end-to-end checklist for cutting a tagged release. Last updated for
feature 011 (nightly mikebom rebuild — appended as §9).

## 0. Prerequisites (one-time)

- You have **push access** to `kusari-oss/mikebom-operator` AND the ability to
  create tags on the `main` branch.
- You have `cosign`, `helm`, `kind`, `gh`, and `oras` installed locally for
  post-publish verification + rollback.
- Sigstore Fulcio + Rekor are operational. Check
  [status.sigstore.dev](https://status.sigstore.dev/) if you suspect issues.

## 1. Pre-tag

1. Switch to `main` and pull the latest:
   ```sh
   git checkout main && git pull origin main
   ```
2. Decide the version. Use semver with optional pre-release suffix:
   `v0.1.0-alpha.1`, `v0.1.0-rc.1`, `v0.1.0`, `v0.2.0-alpha.1`, etc.
3. Update **three** files to the chosen version (without the leading `v`):
   - `Cargo.toml` workspace `[workspace.package] version = "..."`
   - `charts/mikebom-operator/Chart.yaml` `version: ...`
   - `charts/mikebom-operator/Chart.yaml` `appVersion: "..."`
4. Verify locally:
   ```sh
   cargo test --workspace
   ```
   In particular, `cargo test --test version_consistency` MUST pass — that's
   the in-repo invariant catching drift between the three files.
5. Verify chart shape:
   ```sh
   helm lint charts/mikebom-operator/
   cargo test --test crd_drift
   ```
6. Open a PR with the version bumps and a title like
   `chore(release): bump to v0.1.0-alpha.1`. Merge after review.

## 2. (Optional) Dry-run rehearsal

Before the real tag push, you can rehearse the entire release pipeline
against any commit on `main` without publishing anything:

1. Go to
   [Actions → Release](https://github.com/kusari-oss/mikebom-operator/actions/workflows/release.yml)
2. Click **Run workflow**.
3. Pick the branch (typically `main`).
4. Confirm **dry_run** is checked (it is by default).
5. **Run workflow**.

In dry-run mode, the workflow:
- **Skips** the `versions` pre-flight (a workflow_dispatch from a branch like
  `main` doesn't have a valid `vX.Y.Z` tag — gating the check is the right
  call). A `::notice::` is emitted explaining why.
- Builds the multi-arch image but does NOT `docker push`.
- Installs cosign but does NOT `cosign sign` or `cosign attest`.
- **Skips** the SBOM generation step (anchore/sbom-action scans by-digest
  from a registry, which requires a pushed image).
- Packages the chart with `helm package` but does NOT `helm push`.
- Skips the GitHub Release page job entirely.

The dry-run is the workflow's self-test for *infrastructure* (yq installs,
docker multi-arch buildx works, helm package succeeds, cosign-installer SHA
resolves). It does NOT exercise SBOM scanning or signing — those require
real registry artifacts and are only validated on a real tag push.

## 3. Tag push

```sh
git checkout main && git pull
git tag v0.1.0-alpha.1
git push origin v0.1.0-alpha.1
```

The Release workflow fires within ~10 seconds. Wait for it to complete
(typically 8–12 minutes for a clean run, 15-minute budget per FR-013).

Monitor at:
https://github.com/kusari-oss/mikebom-operator/actions/workflows/release.yml

## 4. Post-publish smoke test

After the workflow finishes successfully, verify the artifacts.

### 4.1 Image signature

```sh
cosign verify \
  --certificate-identity-regexp '^https://github.com/kusari-oss/mikebom-operator/' \
  --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' \
  ghcr.io/kusari-oss/mikebom-operator:v0.1.0-alpha.1
```

Expect zero exit. Output includes the signature payload and Rekor log
index.

### 4.2 SBOM attestation

```sh
cosign verify-attestation --type cyclonedx \
  --certificate-identity-regexp '.*' \
  --certificate-oidc-issuer-regexp '.*' \
  ghcr.io/kusari-oss/mikebom-operator:v0.1.0-alpha.1 | jq -r '.payload' | base64 -d | jq '.predicate.components | length'
```

Should print a positive integer (the count of SBOM components). The chain:
`cosign verify-attestation` → DSSE envelope → base64-decode payload →
extract `.predicate.components` → count.

### 4.3 Chart signature

```sh
cosign verify \
  --certificate-identity-regexp '.*' \
  --certificate-oidc-issuer-regexp '.*' \
  ghcr.io/kusari-oss/charts/mikebom-operator:0.1.0-alpha.1
```

Note: chart tag does NOT have the `v` prefix. The git tag is `v0.1.0-alpha.1`;
the chart OCI tag is `0.1.0-alpha.1`.

### 4.4 Live install on kind

```sh
kind create cluster --name release-smoke
helm install mikebom-operator \
  oci://ghcr.io/kusari-oss/charts/mikebom-operator \
  --version 0.1.0-alpha.1 \
  -n kusari-operator --create-namespace --wait

kubectl get pods -n kusari-operator
# Expected: mikebom-operator-<hash> Running 1/1
kind delete cluster --name release-smoke
```

## 5. Re-tag idempotency smoke (FR-014 / SC-006)

Feature 010 designed the workflow to be idempotent for re-runs at the same
tag. Verify this on the first release (and any time the workflow changes):

```sh
git push --force origin v0.1.0-alpha.1
```

Wait for the workflow to complete a second time, then verify:

1. **Exactly one** GitHub Release page at the tag (not duplicated):
   ```sh
   gh release list --limit 5 | grep v0.1.0-alpha.1 | wc -l
   # Expected: 1
   ```
2. **Image signature still passes** (cosign sign is idempotent for the same
   digest):
   ```sh
   cosign verify \
     --certificate-identity-regexp '.*' --certificate-oidc-issuer-regexp '.*' \
     ghcr.io/kusari-oss/mikebom-operator:v0.1.0-alpha.1
   # Expected: zero exit, possibly a new Rekor entry (cosign creates a fresh
   # transparency-log entry but the underlying signature stays valid).
   ```
3. **SBOM attestation still passes**:
   ```sh
   cosign verify-attestation --type cyclonedx \
     --certificate-identity-regexp '.*' --certificate-oidc-issuer-regexp '.*' \
     ghcr.io/kusari-oss/mikebom-operator:v0.1.0-alpha.1
   ```

If any of (1)/(2)/(3) regresses, **stop** and open an issue. Idempotency is
a stated contract invariant; a regression means the workflow has a real bug.

## 6. Rollback (partial publish or failed smoke)

If any step from §4 fails, or if a release needs to be retracted:

1. **Delete the GitHub Release page**:
   ```sh
   gh release delete v0.1.0-alpha.1 --yes
   ```
2. **Delete the operator image from GHCR**:
   ```sh
   # Find the version ID
   gh api -X GET "/orgs/kusari-oss/packages/container/mikebom-operator/versions" \
     | jq -r '.[] | select(.metadata.container.tags[]? == "v0.1.0-alpha.1") | .id'
   # Delete by ID (replace <id>)
   gh api -X DELETE \
     "/orgs/kusari-oss/packages/container/mikebom-operator/versions/<id>"
   ```
3. **Delete the chart from GHCR** (requires `oras` CLI):
   ```sh
   oras manifest delete ghcr.io/kusari-oss/charts/mikebom-operator:0.1.0-alpha.1
   ```
4. **Delete the git tag** (local + remote):
   ```sh
   git tag -d v0.1.0-alpha.1
   git push --delete origin v0.1.0-alpha.1
   ```
5. Fix the underlying issue. Re-tag from scratch (§3).

## 7. Communication

- Post the Release URL to the maintainer Slack channel.
- Open a Discussion thread on GitHub if this is a major release (v1.0+).
- For alpha/beta tags, communicate via the project README + the Release page
  only — no broader announcement.

## 8. Known limitations (carried forward from v0.1.0-alpha.1)

- **Cron timezone**: UTC only. Per-CR timezone field is a future feature.
- **SLSA-L3 build provenance**: not yet integrated. Cosign + GitHub OIDC
  certificate provides L2-equivalent provenance via the Rekor log entry.
- **Automated post-publish E2E smoke**: this runbook is the source of
  truth; CI doesn't yet automate §4.4's kind install. Future feature can.
- **Multi-arch**: linux/amd64 + linux/arm64 only. No s390x, no FreeBSD.
- **No automated changelog generation**: the Release body is a static
  template populated with tag-specific links. Manual notes go in a PR
  description, not the Release page.

## 9. Nightly rebuild workflow (feature 011)

At 03:17 UTC every day, `.github/workflows/nightly-mikebom-bump.yml`
inspects mikebom's release surface and, if a newer alpha is available,
opens a PR that bumps the operator's mikebom pin + regenerates the CRD
+ bumps the operator's own version in structural lockstep. The PR
auto-merges on green CI + Kusari Inspector, and
`.github/workflows/tag-on-nightly-merge.yml` pushes the resulting
operator tag — which fires `release.yml` (this runbook's §§1-4)
unchanged.

### 9.0 First-time setup

Run once, then never again:

```sh
gh label create nightly-mikebom-bump \
  --repo kusari-oss/mikebom-operator \
  --color 0e8a16 \
  --description "Auto-opened by nightly-mikebom-bump.yml"

gh label create nightly-mikebom-bump/cleared \
  --repo kusari-oss/mikebom-operator \
  --color ffd700 \
  --description "Maintainer override — excludes closed PR from known-bad set"

gh label create nightly-mikebom-bump/failure \
  --repo kusari-oss/mikebom-operator \
  --color d93f0b \
  --description "Nightly workflow failure — auto-filed with de-dup"
```

Then enable **two** repo settings — both required for the nightly to
successfully open + auto-merge PRs:

- **Settings → General → Pull Requests → Allow auto-merge** (checkbox)
- **Settings → Actions → General → Workflow permissions → "Allow GitHub
  Actions to create and approve pull requests"** (checkbox, near the
  bottom of the page). Off by default — `gh pr create` from a workflow
  fails with `GraphQL: GitHub Actions is not permitted to create or
  approve pull requests` without this. Discovered during the first
  real-run rehearsal — see PR #26.

Verify the default `GITHUB_TOKEN` can auto-merge against `main`'s
branch protection; if it can't, escalate per §9.6.

### 9.1 What the nightly does

Reads the current mikebom pin from
`charts/mikebom-operator/values.yaml`. Queries the highest
`v0.1.0-alpha.*` release on `kusari-oss/mikebom`. Verifies the
multi-arch image manifest exists on ghcr.io. Checks that no prior
bump PR is still open (FR-018) and that the target alpha isn't in
the derived known-bad set (FR-017). If all checks pass and the target
is strictly newer, bumps every live reference in the repo (12 files
— same set the manual `v0.1.0-alpha.57` bump touched), regenerates
the CRD via `mikebom-operator-ctl`, bumps the operator's own version
in structural lockstep across `Cargo.toml` + `Chart.yaml`, opens a
labeled PR, and enables `gh pr merge --auto --squash`. On merge,
`tag-on-nightly-merge.yml` detects the commit's `Nightly-Bump-Target:`
trailer and pushes the corresponding operator tag.

### 9.2 Dry-run rehearsal

Before enabling the schedule for the first time (or after any change
to the scripts), rehearse against production repo state without
touching origin:

```sh
gh workflow run nightly-mikebom-bump.yml -f dry_run=true
gh run watch
```

Expected: `::notice::current_pin=…` + `[dry_run] Would push branch …`
notices, zero commits/branches on origin, zero PRs opened.

### 9.3 First real-run inspection checklist

After the first non-dry run opens a PR:

- [ ] PR title matches `chore(nightly): bump mikebom to v0.1.0-alpha.<M> and operator to v0.1.0-alpha.<N>`
- [ ] PR body has ✅ verification checkmarks + mikebom release-notes excerpt
- [ ] PR label is `nightly-mikebom-bump`
- [ ] PR branch is `automation/nightly-bump/<mikebom_tag>`
- [ ] HEAD commit message contains `Nightly-Bump-Target: <mikebom_tag>`
- [ ] Diff touches ONLY the 12-file live surface + `Cargo.toml` +
  `Chart.yaml` + regenerated CRD (does NOT touch `specs/003-*` or
  `specs/004-*`)
- [ ] CI + Kusari Inspector run and pass
- [ ] After auto-merge, `tag-on-nightly-merge.yml` fires and pushes
  the tag
- [ ] `release.yml` produces signed artifacts identical in shape to
  §4 above; run §4's cosign-verify commands against the new tag

### 9.4 Common operational actions

**Hold the nightly** — Actions tab → nightly-mikebom-bump → "…" →
Disable workflow. Re-enable when ready. Workflow is stateless.

**Clear a known-bad marker** — either reopen the closed unmerged PR
(`gh pr reopen <n>`) or label it cleared:

```sh
gh pr edit <n> --add-label nightly-mikebom-bump/cleared
```

**Force a bump on-demand** — `gh workflow run nightly-mikebom-bump.yml`
(uses `dry_run=false` by default).

**Cancel a bump PR mid-flight** — `gh pr close <n>` closes without
merging. The next nightly treats that mikebom target as known-bad; if
that's not desired, also apply the `/cleared` label above.

**Roll back a bad release** — out of scope for the nightly. Use §6
above.

### 9.5 Trust boundaries

- Nightly can push commits to bot branches, open PRs, apply labels,
  file/comment issues, enable auto-merge (via `GITHUB_TOKEN`).
- Nightly CANNOT push directly to `main` (branch protection).
- Tag workflow can push tags. It CANNOT overwrite existing tags
  (idempotency check).
- Neither workflow signs anything. Signing remains cosign-keyless-OIDC
  via `release.yml` on tag push (feature 010).
- Neither workflow can bypass Kusari Inspector or CI. Auto-merge fires
  only when both are green.

### 9.6 Troubleshooting

| Symptom | Likely cause | Fix |
|--------|-------------|-----|
| No PR opened despite mikebom moving | Prior unresolved PR (FR-018) | `gh pr list --label nightly-mikebom-bump --state open` — resolve it |
| PR opened but auto-merge didn't enable | `GITHUB_TOKEN` can't auto-merge on protected branch | Repo Settings → General → Allow auto-merge. Consider a GitHub App if it still fails |
| Auto-merge fires but tag never pushed | `Nightly-Bump-Target:` trailer missing from merged commit | Verify auto-merge strategy is `--squash` (trailer survives squash; merge commits shift HEAD) |
| Duplicate failure issues piling up | De-dup search failing | Inspect `gh issue list --label nightly-mikebom-bump/failure`; broaden the `--search` term in the workflow |
| Nightly no-ops when mikebom clearly moved | Known-bad set incorrectly matching | Manually run detection: `bash .github/scripts/nightly-detect.sh 2>&1` — inspect derived known-bad list |
| Tag workflow fires on non-bump merges | Trailer accidentally present in a manual commit | Idempotency check in T006's script prevents damage; still audit the commit |
