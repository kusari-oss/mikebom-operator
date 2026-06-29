# mikebom-operator release runbook

The end-to-end checklist for cutting a tagged release. Last updated for
feature 010 (v0.1.0-alpha.1 pipeline).

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
- Runs the `versions` pre-flight against `GITHUB_REF_NAME` (which is `main`
  on a branch dispatch — the check will likely complain about the tag-format
  mismatch, but the build + SBOM generation steps run regardless).
- Builds the multi-arch image but does NOT `docker push`.
- Generates the CycloneDX SBOM.
- Installs cosign but does NOT `cosign sign` or `cosign attest` (no
  signature wasted on a non-published image).
- Skips `helm push` and the GitHub Release page.

The dry-run job is the workflow's self-test. If anything is broken (yq
missing, sbom-action failing, etc.) you find out before tagging.

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
