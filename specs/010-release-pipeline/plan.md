# Implementation Plan: v0.1.0-alpha.1 release pipeline

**Branch**: `010-release-pipeline` | **Date**: 2026-06-29 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/010-release-pipeline/spec.md`

## Summary

Extend the existing `.github/workflows/release.yml` to publish a signed, SBOM-attested operator image and signed Helm chart on tag push. A new `versions` pre-flight job parses the git tag, then asserts the workspace `Cargo.toml` version, `Chart.yaml` `version`, and `Chart.yaml` `appVersion` all equal the tag minus the `v` prefix — failing the workflow before any push if any drift is detected (US3). The existing `image` job gains cosign keyless signing of the image (by digest, not tag) and a CycloneDX SBOM attestation via `anchore/sbom-action`. The existing `chart` job packages the chart with `version`/`appVersion` set from the tag, pushes to OCI, and cosign-signs the chart artifact. A new `release-notes` job creates (or updates) the GitHub Release page with artifact links and verification commands. Pre-feature, `charts/mikebom-operator/templates/deployment.yaml` is updated to use `{{ .Chart.AppVersion }}` as the image-tag default (instead of `{{ .Values.image.tag }}`'s hard-coded value), so chart lockstep is structural and not a maintenance burden. Three new GH Actions are added: `sigstore/cosign-installer`, `anchore/sbom-action`, `softprops/action-gh-release` — all SHA-pinned per the project's existing convention (Scorecard Pinned-Dependencies, Kusari Inspector).

## Technical Context

**Language/Version**: N/A — this feature is GitHub Actions YAML + a small shell pre-flight script, not Rust code. The Rust workspace's version field is *consumed* (read for version-stamping verification), not changed in nature.

**Primary Dependencies** (all GitHub Actions, SHA-pinned per FR-011):
- `sigstore/cosign-installer@<latest-sha>` (v3.x) — installs cosign CLI in the runner. Maintained by the Sigstore project.
- `anchore/sbom-action@<latest-sha>` (v0.x) — wraps Syft to produce a container SBOM (CycloneDX format). Outputs the SBOM file as a workflow artifact for the cosign-attest step.
- `softprops/action-gh-release@<latest-sha>` (v2.x) — creates/updates a GitHub Release at the workflow's tag with custom body + (optionally) file attachments.
- Existing actions in `release.yml` (`actions/checkout`, `docker/setup-qemu-action`, `docker/setup-buildx-action`, `docker/login-action`, `docker/build-push-action`, `azure/setup-helm`) — already SHA-pinned. Bumps to newer SHAs deferred to a separate dependency-update PR.

**Storage**: N/A — no persistent storage; everything is in-flight workflow state.

**Testing**:
- **Unit tests**: a tiny Rust unit test in a new `crates/operator/tests/version_consistency.rs` (integration test of the `operator` crate) that reads `Cargo.toml` workspace version, `Chart.yaml` `version`, and `Chart.yaml` `appVersion`, and asserts they all match. Runs in every `cargo test --workspace` invocation — catches drift on PRs before any release attempt.
- **Shell smoke test**: a `.github/scripts/check-versions.sh` script invoked by the workflow's `versions` job. Reads tag from `GITHUB_REF_NAME`, strips `v`, asserts vs the three in-repo version strings. Same logic as the Rust test but at workflow time, against the *tag* (not just the workspace).
- **Workflow integration "test"**: an opt-in pre-release dry-run (`workflow_dispatch` trigger added to `release.yml` that runs everything except the push steps when triggered manually). Lets a maintainer rehearse before the first real tag. Documented in the release runbook.
- **Release smoke (manual, post-publish)**: a documented runbook checklist: `cosign verify ...`, `cosign verify-attestation --type cyclonedx ...`, `helm pull oci://...`, `helm install` against a kind cluster, assert `Ready=True`. Tracked in `docs/release-runbook.md` (new file).

**Target Platform**: GitHub-hosted `ubuntu-latest` runners. amd64 + arm64 multi-arch image builds via QEMU + Buildx (already configured in `release.yml`).

**Performance Goals**: end-to-end workflow ≤ 15 minutes (SC-001 / FR-013). Dominant cost is the multi-arch Rust build; cosign signing + SBOM generation each add ≤ 30 seconds.

**Constraints**:
- Constitution VII (Helm Chart Lockstep): every tagged operator image MUST ship with a matching chart version. The `versions` pre-flight job is the enforcement point.
- Saved feedback memory (`feedback_pin_third_party_deps.md`): every new GH Action MUST be SHA-pinned with a `# <tag>` comment. T013 grep check verifies.
- No long-lived secrets: cosign signing uses the GitHub Actions OIDC token (`id-token: write` permission per job), not a key file. Sigstore's Fulcio issues a short-lived signing certificate; Rekor logs the signature publicly.
- The `release.yml` workflow's top-level `permissions: contents: read` default stays in place; per-job permission opt-ins are explicit (existing pattern).
- No automatic Cargo.toml/Chart.yaml *modification* by the workflow — versions are committed by the maintainer before tagging. The workflow only *verifies* alignment.

**Scale/Scope**: one release tag per cycle. v0.x cadence is irregular (per-feature releases), v1.0 will move to a more predictable schedule. v0.1.0-alpha.1 is the first end-to-end published release; the workflow MUST work cleanly on it.

## Constitution Check

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Pure Rust where reasonable | PASS / N/A | Release infra is GH Actions YAML, not Rust. The one Rust artifact (the version-consistency integration test) uses only `std::fs` + `serde_yaml` (workspace dep). |
| II. USE not EMBED (NON-NEGOTIABLE) | PASS / N/A | Release doesn't touch mikebom. |
| III. Fail Closed on RBAC (NON-NEGOTIABLE) | PASS / N/A | No runtime RBAC. GitHub permissions follow least-privilege: top-level `contents: read`, per-job opt-ins (`packages: write` for image/chart push, `id-token: write` for cosign keyless). |
| IV. CRD Backward Compatibility | PASS / N/A | No CRD changes. |
| V. SBOM-Format Agnostic | PASS | The release SBOM uses CycloneDX, matching the operator's default scan format (feature 003's `ScanFormat::CyclonedxJson` default). Could also be SPDX; CDX chosen for symmetry. |
| VI. Hermetic E2E Tests (NON-NEGOTIABLE) | PASS | Release infra doesn't change reconciler logic. Existing `ci.yml` runs all gated E2Es on every PR (and on `main`); the release pipeline triggers on tags that point at PR-merged commits, so the merged code has already passed all E2Es. A post-release manual smoke test (runbook item) verifies the published artifacts work end-to-end. |
| VII. Helm Chart Lockstep | PASS | The `versions` pre-flight job is the *enforcement* of lockstep. Chart `appVersion` drives the deployment's image tag (template update: `{{ .Chart.AppVersion }}` default). The chart CRD YAML stays generated from the Rust struct (feature 001) — unchanged by this feature. |

All gates pass. No `## Complexity Tracking` section needed.

## Project Structure

### Documentation (this feature)

```text
specs/010-release-pipeline/
├── plan.md                              # this file
├── spec.md                              # spec (no clarifications were needed)
├── research.md                          # Phase 0: 7 decisions (cosign action, SBOM tool, attestation format, release-notes action, version-check script shape, chart appVersion templating, smoke-test runbook)
├── data-model.md                        # Phase 1: workflow job graph + version-string entity + FR→test mapping
├── quickstart.md                        # Phase 1: maintainer release runbook (pre-tag checklist + post-publish smoke + rollback procedure)
├── contracts/
│   └── release-workflow.md              # Internal contract: workflow inputs/outputs + job dependencies + failure semantics
└── tasks.md                             # /speckit-tasks output (not created here)
```

### Source / config files modified or added

```text
.github/
├── workflows/
│   └── release.yml                      # MODIFY: add `versions` job (pre-flight), extend `image` job with cosign signing + SBOM attestation, extend `chart` job with cosign signing, add `release-notes` job
└── scripts/
    └── check-versions.sh                # NEW: shell pre-flight that reads GITHUB_REF_NAME, strips `v`, asserts vs Cargo.toml + Chart.yaml. Invoked by the workflow's `versions` job. ~40 lines, POSIX-compatible.

charts/mikebom-operator/
├── Chart.yaml                           # MODIFY: bump `version: 0.1.0` → `0.1.0-alpha.1` so `version` and `appVersion` align (currently they disagree — Chart.yaml line 5 vs 6).
├── templates/
│   └── deployment.yaml                  # MODIFY: change `image: "{{ .Values.image.repository }}:{{ .Values.image.tag }}"` to use `{{ .Values.image.tag | default .Chart.AppVersion }}` so the chart's appVersion is the structural source of truth for which operator image gets pulled.
└── values.yaml                          # MODIFY: remove the hard-coded `image.tag: v0.1.0-alpha.1` default. The chart now derives the tag from `Chart.AppVersion` by default; admins can still override `image.tag` for pinning to a different version.

Cargo.toml                               # MODIFY: bump workspace `version = "0.1.0"` → `"0.1.0-alpha.1"`. Aligns with chart + future git tag.

crates/operator/tests/
└── version_consistency.rs               # NEW: integration test asserting Cargo.toml workspace version == Chart.yaml `version` == Chart.yaml `appVersion`. Runs on every `cargo test --workspace`, catching drift on PRs. ~50 lines.

docs/
├── release-runbook.md                   # NEW: maintainer-facing checklist — pre-tag steps, tag push procedure, post-publish smoke checks (cosign verify, cosign verify-attestation, helm pull + install on kind), rollback procedure for partial-publish state.
└── architecture.md                      # NO CHANGE — release infrastructure is operationally separate from operator architecture.
```

**Structure Decision**:

The implementation lives **entirely outside the operator crate** except for the version-consistency integration test (which is in the operator crate's `tests/` directory so it runs on every `cargo test` invocation). Three rationales:

1. **Release tooling is GH Actions territory**: the spec's USPs (signed image, SBOM attestation, signed chart, GitHub Release page) are all delivered via workflow YAML + a single shell script. Adding Rust code for any of this would be over-engineering.

2. **One Rust artifact, one purpose**: the version-consistency test catches drift early — on every PR, not just at release time. It's the only Rust file added by this feature, and it's a tiny integration test (~50 lines).

3. **Chart structural lockstep**: changing the deployment template to derive `image.tag` from `Chart.AppVersion` (instead of `Values.image.tag`) makes the chart self-consistent by construction — admins who override no values still get the correct operator image. This is a one-line template change with outsized correctness benefit.

## Phase 0: Outline & Research

Research artifact: [research.md](./research.md). The 7 decisions it records:

1. **Cosign action**: `sigstore/cosign-installer@<sha>` (v3.x). Installs the cosign CLI; subsequent `cosign sign` / `cosign attest` calls run as ordinary `run:` steps. Alternative: invoking cosign from a container action (rejected — more friction, less control over CLI flags).

2. **SBOM tool**: `anchore/sbom-action@<sha>` (v0.x, wraps Syft). Outputs CycloneDX JSON to a workflow artifact, which the next step passes to `cosign attest --type=cyclonedx`. Alternatives considered: invoking Syft directly via `curl + sh` (rejected — defeats SHA-pinning unless we vendor the Syft binary), `actions-bake-action` with Buildx attestations (rejected — couples SBOM to Buildx version and the CycloneDX schema Buildx emits is ahead of cosign's verify-attestation parser as of 2026Q2).

3. **Attestation format**: CycloneDX. Matches the operator's default `ScanFormat::CyclonedxJson` from feature 003. Alternative: SPDX (also fine; CDX is project default). The format is verified via `cosign verify-attestation --type=cyclonedx`.

4. **Release-notes action**: `softprops/action-gh-release@<sha>` (v2.x). Creates/updates the Release at the workflow's tag with a custom body. Idempotent (FR-014). Alternative: `gh release create` via gh CLI in a `run:` step (rejected — more boilerplate for body templating and re-run idempotency).

5. **Version-check script shape**: a POSIX shell script `.github/scripts/check-versions.sh` (~40 lines). Reads `GITHUB_REF_NAME`, strips `v`, then:
   - Parses `Cargo.toml` workspace `version` (via `awk` or `grep`).
   - Parses `Chart.yaml` `version` and `appVersion` (via `yq` — installed via `apt-get -y install yq` or `pip install yq`; or a small awk script if we want zero deps).
   - Asserts all three equal the stripped tag.
   - Exits non-zero with a clear diff on mismatch.
   The Rust integration test (T-XX) covers the same logic at `cargo test` time (no tag involved — just the three in-repo strings agreeing). The shell script adds the tag-vs-strings comparison that only matters at release time.

6. **Chart `appVersion` templating**: change `templates/deployment.yaml` line 20 from `{{ .Values.image.tag }}` to `{{ .Values.image.tag | default .Chart.AppVersion }}`, and remove the hard-coded `image.tag: v0.1.0-alpha.1` from `values.yaml`. After this change: an admin who passes no `--set` overrides gets the operator image whose tag matches `Chart.AppVersion` — structural lockstep. An admin who *wants* to pin to a specific image tag (e.g., a digest, or testing a fork build) can still set `image.tag` explicitly. Best practice in Helm chart authoring.

7. **Smoke-test runbook**: `docs/release-runbook.md` (new file) captures the maintainer-facing checklist: pre-tag steps (rebase main, verify `cargo test --workspace` passes locally), tag-push steps (`git tag v0.1.0-alpha.1 && git push origin v0.1.0-alpha.1`), post-publish smoke (cosign verify of image, cosign verify-attestation of SBOM, `helm pull` + `helm install` on kind, assert CR reaches Ready), rollback procedure (delete GitHub Release + delete image tag + delete chart tag — three manual `gh`/`oras` commands). Documented as a runbook because automating the smoke is overkill for v0.1.0-alpha.1 (the first release).

**Output**: research.md with all 7 decisions resolved. No `NEEDS CLARIFICATION` markers remain.

## Phase 1: Design & Contracts

**Prerequisites**: research.md complete.

### Data model

[data-model.md](./data-model.md) captures:

- **Version-string entity (in-repo)**: three places encode the same value — `Cargo.toml` workspace `version`, `Chart.yaml` `version`, `Chart.yaml` `appVersion`. The Rust integration test + the shell pre-flight together enforce all three agree.

- **Git tag (input)**: `GITHUB_REF_NAME` in workflow context. Pattern `v<semver>`. Pre-flight strips the `v` and asserts the remainder matches the three in-repo strings.

- **Workflow job graph (new)**:

  ```
  versions (NEW) ─┬─▶ image (existing, extended)
                  │       │ build + push (existing)
                  │       │ cosign sign image (NEW)
                  │       │ SBOM attest (NEW)
                  │       ↓
                  └─▶ chart (existing, extended)
                          │ package + push (existing)
                          │ cosign sign chart (NEW)
                          ↓
                       release-notes (NEW)
                          │ create/update GitHub Release with artifact links
  ```

  - `versions` gates everything: if it fails, no downstream job runs.
  - `image` and `chart` could run in parallel after `versions` (Buildx + helm package are independent). The existing `release.yml` makes `chart` depend on `image` for ordering; we keep this for simpler debugging (when an image push fails, the chart job doesn't waste compute).
  - `release-notes` depends on both — only fires when image + chart are both up.

- **Output artifacts**: image at `ghcr.io/kusari-oss/mikebom-operator:<tag>` (signed + SBOM-attested), chart at `oci://ghcr.io/kusari-oss/charts/mikebom-operator:<tag-without-v>` (signed), GitHub Release at the tag.

- **FR → test mapping**:

  | FR | Test |
  |---|---|
  | FR-001 | Workflow `on: push: tags: [v*]` filter (existing). Manually verified on a test tag push. |
  | FR-002 | Existing `docker/build-push-action` step (no change needed; multi-arch already configured). |
  | FR-003 | Workflow `tags:` parameter on `build-push-action`; manually verified. |
  | FR-004 | New `cosign sign` step; post-publish runbook verification: `cosign verify ...`. |
  | FR-005 | New `anchore/sbom-action` + `cosign attest --type cyclonedx` steps; post-publish runbook verification: `cosign verify-attestation --type cyclonedx ...`. |
  | FR-006 | `versions` pre-flight asserts `Chart.yaml` `version` AND `appVersion` both equal the stripped tag. |
  | FR-007 | Existing `helm push` step (no change). |
  | FR-008 | New `cosign sign` step on the chart's OCI ref; post-publish runbook verification. |
  | FR-009 | New `versions` job (depends on no others, runs first). |
  | FR-010 | New `release-notes` job. |
  | FR-011 | Static: T-XX grep verification that all new `uses:` lines in `release.yml` are SHA-pinned (`@<40-hex>`). Same pattern as feature 008's T035/T036 polish checks. |
  | FR-012 | No change to Dockerfile; pre-flight runbook step verifies digest pins are intact. |
  | FR-013 | Workflow run timing — measured post-tag-push, not auto-asserted. SC-001 budget. |
  | FR-014 | Idempotency by design — cosign sign/attest are no-ops on existing signatures; `softprops/action-gh-release` updates rather than duplicates. Verified by re-running the workflow on the same tag in dry-run mode. |
  | FR-015 | Static: T-XX inspects job-level `permissions:` blocks in `release.yml`. |

### Contracts

[contracts/release-workflow.md](./contracts/release-workflow.md) captures:

- Workflow inputs: `GITHUB_REF_NAME` (tag), repository secrets (`GITHUB_TOKEN` provided automatically, no others).
- Workflow outputs: image OCI ref, chart OCI ref, GitHub Release URL.
- Failure modes (mirrors spec's Edge Cases): version drift → versions job fails before any push; cosign-after-push failure → image/chart job fails, manual re-run; chart-push-after-image-push failure → chart job fails, image already up (acceptable, documented).
- Idempotency invariants: re-running the workflow at the same tag produces the same outputs (versions still match, cosign is no-op, release-notes update is no-op or innocuous body update).

### Agent context update

The project's `CLAUDE.md` currently points at feature 009's plan. Phase 1 updates this to feature 010.

**Output**: data-model.md, contracts/release-workflow.md, quickstart.md, updated `CLAUDE.md`.

## Re-evaluate Constitution Check (post-design)

| Principle | Status | Notes |
|-----------|--------|-------|
| I | PASS / N/A | One small Rust file added (version-consistency integration test); uses only `std::fs` + workspace `serde_yaml`. |
| II | PASS / N/A | No mikebom touch. |
| III | PASS / N/A | Per-job permissions are explicit + minimal. |
| IV | PASS / N/A | No CRD changes. |
| V | PASS | CycloneDX SBOM matches operator default. |
| VI | PASS | Reconciler logic untouched; existing E2E suite continues to run on PRs. |
| VII | PASS | Chart `appVersion` becomes the structural source of truth for image tag via deployment template change; pre-flight job enforces alignment with `Cargo.toml` + git tag. |

All gates still pass post-design. No complexity tracking needed.
