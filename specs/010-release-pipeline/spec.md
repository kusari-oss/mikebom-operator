# Feature Specification: v0.1.0-alpha.1 release pipeline

**Feature Branch**: `010-release-pipeline`

**Created**: 2026-06-29

**Status**: Draft

**Input**: User description: "v0.1.0-alpha.1 release pipeline (feature 010)"

## User Scenarios & Testing *(mandatory)*

### User Story 1 — Tagged release publishes a signed operator image with SBOM (Priority: P1)

A maintainer pushes a git tag like `v0.1.0-alpha.1` to the default branch. Within ~15 minutes, the operator's container image is publicly available at `ghcr.io/kusari-oss/mikebom-operator:v0.1.0-alpha.1`, **cryptographically signed** with cosign keyless OIDC, and ships with a verifiable SBOM attached as an attestation. A downstream user can run `cosign verify ...` and `cosign verify-attestation --type=cyclonedx ...` against the public image and confirm provenance without contacting the maintainer.

**Why this priority**: This is the centerpiece of the release. Without it, there is no v0.1.0-alpha.1 — admins can't `helm install` the operator (no image to pull), and no chain-of-custody evidence exists for the binary. P1 because the entire alpha release blocks on this.

**Independent Test**: Push `v0.1.0-alpha.1-test` (or use a release dry-run workflow if one exists). Within 15 minutes, `docker pull ghcr.io/kusari-oss/mikebom-operator:v0.1.0-alpha.1-test` succeeds, and `cosign verify --certificate-identity-regexp '.*' --certificate-oidc-issuer-regexp '.*' ghcr.io/kusari-oss/mikebom-operator:v0.1.0-alpha.1-test` returns 0.

**Acceptance Scenarios**:

1. **Given** the repository at a commit on the default branch, **When** the maintainer pushes a tag matching `v[0-9]+.[0-9]+.[0-9]+*` (semver with optional pre-release suffix), **Then** the release workflow runs end-to-end and publishes a signed image to `ghcr.io/kusari-oss/mikebom-operator:<tag>` within 15 minutes.
2. **Given** a published image, **When** any user runs `cosign verify` with the published Sigstore certificate identity, **Then** the verification succeeds — the image's signature is cryptographically valid and chain-of-custody traces to the GitHub Actions OIDC identity for this repository.
3. **Given** a published image, **When** any user runs `cosign verify-attestation --type=cyclonedx ...` (or `--type=spdx`), **Then** an SBOM attestation is present and the SBOM payload lists every Rust dependency baked into the operator binary.

---

### User Story 2 — Tagged release publishes a signed Helm chart (Priority: P1)

The same tag push that builds the operator image also packages and publishes a Helm chart to `oci://ghcr.io/kusari-oss/charts/mikebom-operator:<tag>`. The chart's `version` AND `appVersion` MUST match the tag (minus the `v` prefix). The chart is cosign-signed via the same keyless OIDC flow as the image. Admins can run `helm pull oci://ghcr.io/kusari-oss/charts/mikebom-operator --version v0.1.0-alpha.1` and get a verifiable chart whose `appVersion` matches the operator image they're installing.

**Why this priority**: Constitution principle VII (Helm Chart Lockstep) mandates that every tagged operator image ships with a matching chart version. P1 because shipping the image without the chart leaves admins unable to install; shipping mismatched versions violates the constitution.

**Independent Test**: After US1's release publishes, run:

```sh
helm pull oci://ghcr.io/kusari-oss/charts/mikebom-operator --version 0.1.0-alpha.1
tar -xzf mikebom-operator-0.1.0-alpha.1.tgz
grep -E "^(version|appVersion):" mikebom-operator/Chart.yaml
```

Both `version` and `appVersion` MUST read `0.1.0-alpha.1`. `cosign verify` against the chart's OCI ref MUST return 0.

**Acceptance Scenarios**:

1. **Given** a successful image publish (US1), **When** the chart job runs, **Then** the chart is packaged with `Chart.yaml` `version` and `appVersion` both equal to the tag (minus the `v` prefix), and pushed to `oci://ghcr.io/kusari-oss/charts/mikebom-operator:<tag-without-v>`.
2. **Given** a published chart, **When** any user runs `cosign verify` against the chart's OCI ref, **Then** the verification succeeds with the same certificate identity as the image.
3. **Given** a chart pulled at version `0.1.0-alpha.1`, **When** an admin runs `helm install`, **Then** the installed operator pod uses the image tagged `v0.1.0-alpha.1` (the chart's default `image.tag` value matches `appVersion`).

---

### User Story 3 — Version-stamping consistency check (Priority: P2)

Before any artifact is published, a pre-flight check in the release workflow verifies that every version string in the repository matches the git tag. If any drift is detected — `Cargo.toml` workspace version disagrees with `Chart.yaml` `version`, or `appVersion` disagrees, or the tag disagrees with all of them — the workflow fails loudly **before** any push to ghcr.io. No partial releases.

**Why this priority**: Without this, a maintainer can accidentally tag `v0.2.0` while `Cargo.toml` still says `0.1.0`, publish a mislabeled image, and confuse downstream users. P2 because the workflow still completes a release if versions happen to be consistent — but the check is the safety net.

**Independent Test**: Modify `charts/mikebom-operator/Chart.yaml` `version` to a value that doesn't match `Cargo.toml`. Run the workflow (or its pre-flight check standalone). The pre-flight MUST exit non-zero with a clear message naming the drifted files.

**Acceptance Scenarios**:

1. **Given** a tag `v0.1.0-alpha.1` and matching `Cargo.toml` (`version = "0.1.0-alpha.1"`) and `Chart.yaml` (`version: 0.1.0-alpha.1`, `appVersion: "0.1.0-alpha.1"`), **When** the workflow runs, **Then** the pre-flight check passes and the rest of the workflow proceeds.
2. **Given** a tag `v0.2.0` and `Cargo.toml` still reads `version = "0.1.0"`, **When** the workflow runs, **Then** the pre-flight check fails with a message naming `Cargo.toml` as the drifted file, AND no image/chart push occurs.
3. **Given** a tag `v0.1.0-alpha.1` and matching `Cargo.toml` but `Chart.yaml`'s `version: 0.1.0` (drift between `version` and `appVersion`), **When** the workflow runs, **Then** the pre-flight fails with a message naming the chart field.

---

### User Story 4 — Automated GitHub Release with artifact links (Priority: P3)

After the image and chart publish, the workflow creates (or updates) a GitHub Release at the tag. The release body links to the published image and chart, lists the SBOM attestation status, and notes the cosign verification commands so a user can paste-and-verify without reading documentation.

**Why this priority**: P3 because users can find the artifacts via `ghcr.io` browsing without a GitHub Release page, but the release page is the canonical discoverability surface most admins expect. Skipping it isn't a correctness issue but degrades UX.

**Independent Test**: After US1+US2 publish, `gh release view v0.1.0-alpha.1` MUST return a release whose body contains:
- A link to the image ref
- A link to the chart OCI ref
- Verification commands for `cosign verify` and `cosign verify-attestation`

**Acceptance Scenarios**:

1. **Given** a successful image + chart publish, **When** the workflow's release-notes job runs, **Then** a GitHub Release exists at the tag with the artifact links and verification commands in its body.
2. **Given** a re-run of the workflow at the same tag (e.g., maintainer force-re-runs), **When** the release-notes job runs, **Then** the existing GitHub Release is updated (or left unchanged if already correct) — NOT duplicated.

---

### Edge Cases

- **Tag pushed without `v` prefix** (e.g., `0.1.0-alpha.1`): the workflow's existing `tags: [v*]` filter rejects it — no run triggered. Maintainer must use `v0.1.0-alpha.1`. Document in the contributor README.
- **Tag pushed to a feature branch instead of `main`**: workflow still runs (tags don't carry branch context in GitHub Actions). The pre-flight version check is the only guard. Maintainer responsible for tagging the right commit; doc in CONTRIBUTING.md.
- **Tag force-pushed** (e.g., maintainer realizes the wrong commit was tagged and re-tags): the workflow re-runs. Image push to ghcr.io overwrites the previous tag (allowed by GHCR by default). cosign signing creates a new signature; the old signature stays in the registry but is no longer associated with the current image digest. This is acceptable for alpha; document the caveat.
- **`cosign sign` fails after image push succeeded**: the image is in the registry but unsigned. The workflow MUST mark this as a failure and surface a clear error. A retry of the workflow re-signs the existing image (cosign sign is idempotent on the same digest).
- **SBOM generation fails**: image and chart are signed but no SBOM attestation. The workflow MUST surface this as a degraded-but-not-fatal warning OR fail outright. For v0.1.0-alpha.1 we choose **fail outright** — alpha SBOMs are core to the project's identity (it's an SBOM generator).
- **`helm push` fails after image push succeeded**: image is up but chart is not. The workflow fails; the un-shipped chart must be manually published from a maintainer machine OR the workflow re-run after fixing the underlying issue. Document the rollback procedure.
- **Workflow run hits the 15-minute deadline**: the release is still in flight when the budget elapses. The workflow continues but SC-001 fails — surface a warning. Most likely cause is multi-arch build slowness; the optimization is a future feature.
- **OIDC provider unavailable**: cosign keyless signing requires the Fulcio CA + Rekor transparency log. If either is down, signing fails. The workflow MUST retry up to 3 times with exponential backoff before failing. Document Sigstore status page in the runbook.
- **Pre-existing GitHub Release at the tag** (e.g., manually created during testing): the workflow's release-notes step updates the body rather than failing. Idempotent.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: A git tag matching `v*.*.*` (with optional pre-release suffix like `v0.1.0-alpha.1`) pushed to the repository MUST trigger the release workflow.
- **FR-002**: The release workflow MUST build the operator's container image for at least `linux/amd64` (multi-arch is desired but not required for v0.1.0-alpha.1 — the existing `release.yml` already builds amd64 + arm64; this MUST continue to work).
- **FR-003**: The container image MUST be pushed to `ghcr.io/kusari-oss/mikebom-operator:<tag>` where `<tag>` is the literal git tag (e.g., `v0.1.0-alpha.1`).
- **FR-004**: The container image MUST be cryptographically signed via cosign **keyless OIDC** using the GitHub Actions OIDC token. The signature MUST be verifiable with `cosign verify --certificate-identity-regexp '^https://github.com/kusari-oss/mikebom-operator/' --certificate-oidc-issuer 'https://token.actions.githubusercontent.com'`.
- **FR-005**: An SBOM MUST be generated for the operator image (CycloneDX or SPDX format) and attached as a cosign attestation. The SBOM MUST list every direct Rust dependency baked into the binary. The attestation MUST be verifiable with `cosign verify-attestation --type=cyclonedx ...` (or `--type=spdx`).
- **FR-006**: The Helm chart MUST be packaged with `Chart.yaml` `version` and `appVersion` both equal to the git tag minus the `v` prefix (e.g., tag `v0.1.0-alpha.1` → both fields `0.1.0-alpha.1`).
- **FR-007**: The Helm chart MUST be pushed to `oci://ghcr.io/kusari-oss/charts/mikebom-operator:<tag-without-v>` (e.g., `0.1.0-alpha.1`).
- **FR-008**: The Helm chart MUST be cosign-signed using the same keyless OIDC flow as the image (FR-004). Verifiable with `cosign verify` against the chart's OCI ref.
- **FR-009**: A pre-flight check MUST run before any push to ghcr.io and verify: (a) workspace `Cargo.toml` `version` matches the tag minus `v`, (b) `Chart.yaml` `version` matches, (c) `Chart.yaml` `appVersion` matches. If any drift, the workflow MUST fail with a clear message naming the drifted file(s) — no partial publish.
- **FR-010**: After a successful publish, the workflow MUST create (or update) a GitHub Release at the tag. The release body MUST include: (a) the image's OCI ref, (b) the chart's OCI ref, (c) cosign verification commands for both, (d) the SBOM attestation note.
- **FR-011**: Every GitHub Actions step in the new workflow path MUST pin its action to a **commit SHA**, not a tag, matching the project's existing convention (per the saved `feedback_pin_third_party_deps.md` memory). New actions added (cosign-installer, anchore/sbom-action, etc.) MUST follow this rule.
- **FR-012**: Container base images in the operator's Dockerfile MUST remain pinned to manifest-list digests (the existing pattern). Feature 010 does not touch the Dockerfile other than to verify pinning is intact.
- **FR-013**: The workflow MUST complete within 15 minutes for a clean run on the GitHub-hosted `ubuntu-latest` runner. (SC-001's measurable target.)
- **FR-014**: The workflow MUST be idempotent for re-runs at the same tag (e.g., force-push or manual re-run): version check still passes, cosign sign on already-signed image is a no-op, SBOM attestation re-attaches (cosign attest is idempotent), GitHub Release is updated not duplicated.
- **FR-015**: Cosign keyless signing requires `id-token: write` and `packages: write` permissions on the relevant workflow jobs. Workflow-level default permissions MUST stay `contents: read`; per-job opt-ins MUST be explicit (existing pattern in `release.yml`).

### Key Entities

- **Git tag (input)**: human-pushed semver-formatted tag (e.g., `v0.1.0-alpha.1`) that triggers the entire pipeline. Source of truth for the version stamping.
- **Operator container image (output)**: multi-arch (currently amd64+arm64) image at `ghcr.io/kusari-oss/mikebom-operator:<tag>`. Cosign-signed, SBOM-attested.
- **Helm chart OCI artifact (output)**: signed chart at `oci://ghcr.io/kusari-oss/charts/mikebom-operator:<tag-without-v>`. Carries the chart YAML files; references the operator image by tag.
- **Cosign attestation (output)**: SBOM attestation attached to the operator image's digest in the Rekor transparency log + the registry's OCI metadata.
- **GitHub Release page (output)**: human-readable artifact discovery surface at the tag.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A push of `v0.1.0-alpha.1` to the repository results in a published, cosign-verifiable image at `ghcr.io/kusari-oss/mikebom-operator:v0.1.0-alpha.1` within 15 minutes of the push (workflow start to image push complete). (Verifies FR-001, FR-003, FR-004, FR-013.)
- **SC-002**: 100% of published images carry a valid SBOM attestation. A `cosign verify-attestation` call against any released image returns 0. (Verifies FR-005.)
- **SC-003**: A push of `v0.1.0-alpha.1` results in a published, cosign-verifiable Helm chart at `oci://ghcr.io/kusari-oss/charts/mikebom-operator:0.1.0-alpha.1` whose `appVersion` matches `0.1.0-alpha.1`. (Verifies FR-006, FR-007, FR-008.)
- **SC-004**: Attempting to release a tag whose version doesn't match the in-repo version strings (`Cargo.toml`, `Chart.yaml` `version`, `Chart.yaml` `appVersion`) produces a workflow failure with a message naming the drifted file(s) within 30 seconds of the workflow starting. No image or chart push occurs. (Verifies FR-009.)
- **SC-005**: After a successful release, `gh release view <tag>` returns a release page whose body contains the image OCI ref, chart OCI ref, and cosign verification commands. (Verifies FR-010.)
- **SC-006**: A second push (force-push or re-tag) at the same tag MUST NOT produce duplicate GitHub Releases AND MUST NOT corrupt the existing artifacts (the second cosign sign is idempotent against the same image digest). (Verifies FR-014.)
- **SC-007**: A grep of the release workflow file MUST return zero matches for action references that are NOT SHA-pinned. Pattern: `uses: [^@]*@v[0-9]` (tag-based pinning) MUST NOT appear. Only `uses: <action>@<40-hex-sha> # <tag>` is acceptable. (Verifies FR-011 + the saved feedback memory.)

## Assumptions

- **Cosign keyless OIDC is the signing path** (not key-based). GitHub Actions provides an OIDC token in workflow jobs that have `id-token: write` permission; cosign uses this token to obtain a short-lived signing certificate from the Fulcio CA. No long-lived secret to manage. Constitution-aligned with the project's overall "no long-lived credentials" posture.
- **SBOM tool choice**: planning will pick a specific tool — likely `anchore/sbom-action` (which wraps Syft) or `syft` directly. CycloneDX format chosen (matches the operator's default `scanFormat: cyclonedx-json` — see feature 001 — for symmetry; SPDX is a fine alternative but the project ecosystem leans CDX).
- **No SLSA-level-3 build provenance via `slsa-framework/slsa-github-generator` in v0.1.0-alpha.1**. Cosign's keyless signing + the GitHub OIDC certificate together provide SLSA-level-2-equivalent provenance (verifiable build identity + the source repository). Adding the full SLSA generator is a follow-up. Documented as a roadmap item.
- **Multi-arch is preserved, not extended**: the existing `release.yml` already builds linux/amd64 + linux/arm64. v0.1.0-alpha.1 keeps both. Adding linux/s390x or freebsd is out of scope.
- **No automated release notes generation in v0.1.0-alpha.1**: the GitHub Release body is a static template populated with tag-specific data (image ref, chart ref, verification commands). Auto-aggregating PR descriptions or commit history into a changelog is a follow-up.
- **Tag format is `v<semver>`**: matches the project's existing convention. Pre-release suffixes like `-alpha.1`, `-rc.1` are honored. The `v*` filter in `release.yml` already enforces the `v` prefix.
- **No tag-format validator regex beyond `v*`**: the pre-flight version check (FR-009) effectively validates the tag IS semver (because it has to match Cargo.toml/Chart.yaml which are validated by their respective tools). A standalone regex check is optional polish.
- **Pre-flight failure rolls back nothing** because nothing has been pushed yet. The check runs **before** any push job.
- **Maintainer trust model**: any maintainer with push-tag permission can trigger a release. There's no separate "release approver" role in v0.1.0-alpha.1. GitHub branch protection on `main` already controls who can merge; tagging is a logical extension. Future feature could add manual approval via environment protection rules.
- **Constitution VII E2E**: a separate kind-based smoke test would verify the released chart + image can be installed and reach Ready. For v0.1.0-alpha.1 this test is documented as a manual smoke (in the release runbook), not automated. Adding it as automated post-release verification is a follow-up.
- **Existing release.yml structure is the foundation**, not a from-scratch rewrite. Feature 010 ADDS to the existing workflow: pre-flight version check job, cosign signing steps, SBOM attestation step, GitHub Release creation job. Existing image build + chart push steps stay structurally the same (with added signing).
- **Saved memory honored** (`feedback_pin_third_party_deps.md`): every new GH Action added in this feature MUST be pinned to a commit SHA with a `# <tag>` comment. Kusari Inspector will validate this on the PR.
