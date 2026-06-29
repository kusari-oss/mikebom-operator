# Quickstart: v0.1.0-alpha.1 release pipeline

Two perspectives: cluster admin (consuming the published artifacts) and
maintainer (cutting a release).

## Cluster admin: consuming a v0.x release

### Install with cosign verification

```sh
# Pull the chart from ghcr.io (OCI).
helm pull oci://ghcr.io/kusari-oss/charts/mikebom-operator \
  --version 0.1.0-alpha.1

# Verify the chart's cosign signature (keyless OIDC, no key files needed).
cosign verify \
  --certificate-identity-regexp '^https://github.com/kusari-oss/mikebom-operator/' \
  --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' \
  oci://ghcr.io/kusari-oss/charts/mikebom-operator:0.1.0-alpha.1

# Install — the chart's appVersion (0.1.0-alpha.1) drives the operator image
# pulled. No need to override image.tag.
helm install mikebom-operator ./mikebom-operator-0.1.0-alpha.1.tgz \
  -n kusari-operator --create-namespace
```

### Verify the operator image directly

```sh
# Image is signed:
cosign verify \
  --certificate-identity-regexp '^https://github.com/kusari-oss/mikebom-operator/' \
  --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' \
  ghcr.io/kusari-oss/mikebom-operator:v0.1.0-alpha.1

# Image has an SBOM attestation (CycloneDX):
cosign verify-attestation --type cyclonedx \
  --certificate-identity-regexp '.*' \
  --certificate-oidc-issuer-regexp '.*' \
  ghcr.io/kusari-oss/mikebom-operator:v0.1.0-alpha.1
```

Both commands MUST return zero. The SBOM payload (a CycloneDX JSON document)
is inside the attestation envelope; extract it with `jq` if you want the
component list.

### Where to find releases

- **Image**: `ghcr.io/kusari-oss/mikebom-operator:v<semver>`
- **Chart**: `oci://ghcr.io/kusari-oss/charts/mikebom-operator:<semver>` (no `v` prefix on the chart tag — Helm convention)
- **Release page**: `https://github.com/kusari-oss/mikebom-operator/releases/tag/v<semver>` (links + verification commands)
- **Rekor entries**: searchable at `https://search.sigstore.dev/`

## Maintainer: cutting a release

The full runbook lives in `docs/release-runbook.md`. Quick version:

### Pre-tag

1. Make sure `main` is at the commit you want to release.
2. Update `Cargo.toml` workspace `version` + `charts/mikebom-operator/Chart.yaml` `version` + `appVersion` ALL to the same value (e.g., `0.1.0-alpha.1` — no `v` prefix in any of these files).
3. Run `cargo test --workspace`. The `version_consistency` integration test catches drift between the three in-repo strings.
4. Run `helm lint charts/mikebom-operator/` and `cargo test --test crd_drift`. Both must pass.
5. Commit the version bumps (`chore(release): bump to v0.1.0-alpha.1`), open PR, merge after review.

### Tag push

```sh
git checkout main
git pull
git tag v0.1.0-alpha.1
git push origin v0.1.0-alpha.1
```

The release workflow fires automatically on the tag push. Wait ~15 minutes.

### Post-publish smoke

The Release page at `https://github.com/kusari-oss/mikebom-operator/releases/tag/v0.1.0-alpha.1` shows the artifacts. To smoke-test manually:

```sh
# 1. Verify image signature + SBOM attestation
cosign verify --certificate-identity-regexp '.*' --certificate-oidc-issuer-regexp '.*' \
  ghcr.io/kusari-oss/mikebom-operator:v0.1.0-alpha.1
cosign verify-attestation --type cyclonedx \
  --certificate-identity-regexp '.*' --certificate-oidc-issuer-regexp '.*' \
  ghcr.io/kusari-oss/mikebom-operator:v0.1.0-alpha.1

# 2. Verify chart signature
cosign verify --certificate-identity-regexp '.*' --certificate-oidc-issuer-regexp '.*' \
  oci://ghcr.io/kusari-oss/charts/mikebom-operator:0.1.0-alpha.1

# 3. Live install on a kind cluster
kind create cluster --name release-smoke
helm install mikebom-operator oci://ghcr.io/kusari-oss/charts/mikebom-operator \
  --version 0.1.0-alpha.1 -n kusari-operator --create-namespace --wait
kubectl get pods -n kusari-operator   # operator pod should be Running
kind delete cluster --name release-smoke
```

If any step fails, see the rollback procedure below.

### Rollback (partial publish or failed smoke)

```sh
# Delete the GitHub Release
gh release delete v0.1.0-alpha.1 --yes

# Delete the image from ghcr.io (find the version ID first)
gh api -X GET "/orgs/kusari-oss/packages/container/mikebom-operator/versions" \
  | jq '.[] | select(.metadata.container.tags[]? == "v0.1.0-alpha.1") | .id'
gh api -X DELETE \
  "/orgs/kusari-oss/packages/container/mikebom-operator/versions/<id>"

# Delete the chart from ghcr.io
oras manifest delete ghcr.io/kusari-oss/charts/mikebom-operator:0.1.0-alpha.1

# Delete the git tag (local + remote)
git tag -d v0.1.0-alpha.1
git push --delete origin v0.1.0-alpha.1
```

Fix the underlying issue, then re-tag from scratch.

## Contributor: extending the release pipeline

### Adding SLSA-L3 build provenance

When ready:

1. Add `slsa-framework/slsa-github-generator/.github/workflows/builder_docker-based_slsa3.yml@<sha>` as a referenced reusable workflow.
2. Update `release-notes` body template to link to the SLSA provenance file.
3. Document the SLSA verification command in this quickstart's "Cluster admin" section.

### Automating the post-publish smoke test

A future `smoke` job after `release-notes`:

1. `kind create cluster`
2. `helm install` the just-published chart
3. `kubectl wait --for=condition=Ready namespacescan/...`
4. `kind delete cluster`

Estimated 5-10 minutes added to the release wall-clock. Worth doing once
v0.1.0-alpha.{1,2,3} prove the workflow is stable.

### Bumping action SHAs

A separate dependabot or `actions-up` PR cycle. Run `gh actions-up` or a
similar tool to suggest SHA bumps; manually verify each new SHA points at a
released tag of the action (and the tag matches the project's project trust
model — Sigstore-maintained, Anchore-maintained, etc.).
