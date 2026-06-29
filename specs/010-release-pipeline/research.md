# Phase 0 Research: v0.1.0-alpha.1 release pipeline

This document records the decisions feature 010 makes before code lands. Each
decision is short: what we're doing, why, and what we considered. The plan
references each by its number.

## 1. Cosign action

- **Decision**: `sigstore/cosign-installer@<sha>` (v3.x). Installs the cosign
  CLI on the runner; subsequent `cosign sign` and `cosign attest` calls run as
  ordinary `run:` shell steps in the same job. SHA-pinned with the version tag
  in a trailing comment per the project convention.
- **Rationale**: Sigstore-maintained, widely used, exposes the full cosign
  CLI for explicit control of sign-by-digest vs sign-by-tag (we use by-digest
  for immutability). Pure installer — no opinion-laden wrapper.
- **Alternatives**: container action (rejected — more friction, less control
  over CLI flags). Vendored cosign binary in the repo (rejected — version
  drift, large file).

## 2. SBOM tool

- **Decision**: `anchore/sbom-action@<sha>` (v0.x, wraps Syft). Configured to
  scan the just-pushed container image (by digest) and write a CycloneDX JSON
  file to a known path. The next step passes that path to
  `cosign attest --predicate <file> --type cyclonedx`.
- **Rationale**: Most-used SBOM-for-container action in the Sigstore +
  Anchore ecosystems. Syft produces a complete container manifest (binary +
  base image contents + Cargo dependencies). The CDX JSON is the
  cosign-attest-friendly format.
- **Alternatives**: Invoke Syft directly via `curl + sh` (rejected — defeats
  SHA-pinning unless we vendor the binary). Buildx provenance/SBOM attestations
  via `docker/buildx-action` (rejected — couples SBOM to Buildx version, and
  the schema Buildx emits as of 2026Q2 has cosign-verify-attestation parser
  edge cases). `aquasecurity/trivy-action` (rejected — Trivy's primary purpose
  is vuln scanning; SBOM output works but is less standard for cosign attest).

## 3. Attestation format

- **Decision**: CycloneDX JSON (CDX). Verified via
  `cosign verify-attestation --type cyclonedx ...`.
- **Rationale**: Matches the operator's default `ScanFormat::CyclonedxJson`
  from feature 003 — same format the operator emits for *workload* SBOMs,
  symmetry across the project. CDX is also the default for `anchore/sbom-action`.
- **Alternatives**: SPDX 2.3 JSON (also fine; cosign-attest supports both).
  Cyclone-XML (rejected — XML; CDX JSON is the canonical form for cosign-attest).

## 4. Release-notes action

- **Decision**: `softprops/action-gh-release@<sha>` (v2.x). Creates the
  Release if it doesn't exist; updates the body if it does. Idempotent for
  re-runs (FR-014).
- **Rationale**: De-facto standard GH Actions release-creation action.
  Handles draft vs published, prerelease flag (we set true for `-alpha.N`
  tags), and body templating natively.
- **Alternatives**: `gh release create` via the gh CLI in a `run:` step
  (rejected — more boilerplate for body templating + re-run idempotency).
  `actions/create-release` (rejected — deprecated by GitHub, archived repo).

## 5. Version-check script shape

- **Decision**: a POSIX shell script `.github/scripts/check-versions.sh`
  (~40 lines). Reads `GITHUB_REF_NAME`, strips a leading `v`, then:
  1. Extracts `Cargo.toml` workspace `version` via grep + sed
     (`grep -E '^version = ' Cargo.toml | head -1 | sed -E 's/.*"(.+)".*/\\1/'`).
  2. Extracts `Chart.yaml` `version` and `appVersion` via `yq` (installed via
     `pip install yq` in the same workflow job — small dep, used only here).
  3. Asserts all three equal the stripped tag.
  4. Exits non-zero with a clear diff on mismatch.

  The Rust integration test (`crates/operator/tests/version_consistency.rs`)
  exercises the same three-string consistency check but at `cargo test` time
  (no tag involved) — catches drift on PRs.

- **Rationale**: Two layers of defense. Rust test catches drift early (every
  PR); shell script catches *tag-vs-strings* drift only checkable at release
  time. Both share the parsing logic in spirit; both are short enough to be
  obvious. No new runtime deps on the operator side; the workflow installs
  `yq` per run.
- **Alternatives**: A Rust binary that does both checks (rejected — would
  need to compile in the workflow, slowing every release run; also overkill
  for ~40 lines of shell). Pure `awk`/`grep` for Chart.yaml (rejected — YAML
  multi-line scalars and quoting edge cases; yq is small and robust).

## 6. Chart `appVersion` templating

- **Decision**: Change `charts/mikebom-operator/templates/deployment.yaml`
  line 20 from:
  ```yaml
  image: "{{ .Values.image.repository }}:{{ .Values.image.tag }}"
  ```
  to:
  ```yaml
  image: "{{ .Values.image.repository }}:{{ .Values.image.tag | default .Chart.AppVersion }}"
  ```
  Then remove the hard-coded `image.tag: v0.1.0-alpha.1` line from
  `values.yaml`. Result: an admin who passes no `--set` overrides gets the
  operator image whose tag matches `Chart.AppVersion` automatically.
- **Rationale**: Structural enforcement of constitution VII — chart's
  appVersion drives the image pulled. Avoids the maintenance trap of updating
  three places (`Chart.yaml.version`, `Chart.yaml.appVersion`,
  `values.yaml.image.tag`) every release. The `| default` filter preserves
  the existing override capability (admins can still pin to a digest for
  testing).
- **Alternatives**: Keep `image.tag` as the primary, copy `appVersion` into
  `values.yaml` via a release-time `sed` (rejected — auto-modification of
  source files in the workflow is fragile; explicit + structural is better).

## 7. Smoke-test runbook

- **Decision**: a new `docs/release-runbook.md` file captures the
  maintainer-facing checklist:
  - **Pre-tag**: rebase main, run `cargo test --workspace`, run `helm lint
    charts/mikebom-operator/`, confirm `cargo test version_consistency` passes
    against the version you intend to tag.
  - **Tag**: `git tag v<semver> && git push origin v<semver>`. Workflow fires.
  - **Post-publish smoke**:
    1. `cosign verify --certificate-identity-regexp '.*kusari-oss/mikebom-operator/.*' --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' ghcr.io/kusari-oss/mikebom-operator:v<semver>`
    2. `cosign verify-attestation --type cyclonedx --certificate-identity-regexp '.*' --certificate-oidc-issuer-regexp '.*' ghcr.io/kusari-oss/mikebom-operator:v<semver>`
    3. `helm pull oci://ghcr.io/kusari-oss/charts/mikebom-operator --version <semver>`
    4. `cosign verify oci://ghcr.io/kusari-oss/charts/mikebom-operator:<semver>` (chart sig)
    5. `kind create cluster` + `helm install` + `kubectl wait --for=condition=Ready namespacescan/...` (live smoke against a fresh cluster).
  - **Rollback**: `gh release delete v<semver>`, `gh api -X DELETE
    /repos/kusari-oss/mikebom-operator/packages/container/mikebom-operator/versions/<id>` (delete tagged image), `oras delete oci://ghcr.io/kusari-oss/charts/mikebom-operator:<semver>` (delete chart). Three commands; documented with each command's expected output.

- **Rationale**: First release doesn't need automated post-publish E2E — the
  manual checklist gives the maintainer ground truth and surfaces any
  weirdness. Once we've shipped v0.1.0-alpha.{1,2,3} and confirmed the
  workflow is stable, a future feature can automate the smoke-test job.
- **Alternatives**: Automated post-publish kind E2E job in the workflow
  (deferred — adds 5-10 minutes to the release wall-clock, complicates
  rollback when the smoke fails). External canary deployment (out of scope for
  v0.x).

## Cross-feature compatibility

- **Feature 001 (CRD drift)**: Untouched. The chart CRD YAML is regenerated
  from the Rust struct on every PR (per feature 001's drift check); release
  workflow consumes whatever the merged main branch produces.
- **Features 002–009**: All runtime/reconciler features are untouched. The
  release pipeline operates on the merged main branch's artifacts.
- **Future SLSA-L3**: When ready, add `slsa-framework/slsa-github-generator`
  as a fourth signing/attestation job. The current cosign + GH OIDC
  certificate provides L2-equivalent provenance — the chain `tag → workflow run
  → certificate → image digest` is already publicly verifiable via Rekor.
