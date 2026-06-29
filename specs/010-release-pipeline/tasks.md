---

description: "Task list for feature 010: v0.1.0-alpha.1 release pipeline"
---

# Tasks: v0.1.0-alpha.1 release pipeline

**Input**: Design documents from `/specs/010-release-pipeline/`

**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/release-workflow.md, quickstart.md

**Tests**: Included. Same TDD-style discipline as features 001–009 — Rust integration test for the version-consistency invariant + grep-style polish checks for the GH Actions YAML.

**Organization**: User-story ordering reflects business priority (US1+US2 P1, US3 P2, US4 P3). Implementation order is dependency-driven — US3's version pre-flight is landed FIRST because both US1 (image) and US2 (chart) jobs `needs: [versions]` once US3 ships. The user-story-phase split tags tasks by their story even though the implementation order isn't priority-strict.

## Format: `[ID] [P?] [Story?] Description with file path`

- **[P]**: Can run in parallel (different files, no in-flight dependency).
- **[Story]**: Required on Phase 3+ tasks (US1 / US2 / US3 / US4).

## Path Conventions

GitHub Actions workflow at `.github/workflows/release.yml`. Helper script at `.github/scripts/check-versions.sh`. Rust integration test at `crates/operator/tests/version_consistency.rs`. Chart files at `charts/mikebom-operator/{Chart.yaml,values.yaml,templates/deployment.yaml}`. All paths repo-relative.

---

## Phase 1: Setup (Version + Chart Lockstep)

**Purpose**: Bring the in-repo version strings into agreement, and change the chart's deployment template to derive the image tag from `Chart.AppVersion` so lockstep is structural.

**⚠️ NOTE**: These changes will INTENTIONALLY fail `cargo test` on the new `version_consistency` integration test until ALL of T001–T004 are committed together — the three version strings must change as a unit.

- [X] T001 Bump workspace `Cargo.toml` `[workspace.package] version` from `"0.1.0"` to `"0.1.0-alpha.1"` so it matches the future first-tagged release in `Cargo.toml`
- [X] T002 Bump `charts/mikebom-operator/Chart.yaml` `version:` from `0.1.0` to `0.1.0-alpha.1` (closes L1 from /speckit-analyze: the `appVersion:` field is **already** `"0.1.0-alpha.1"` — verify and leave unchanged). The goal is `version == appVersion == Cargo.toml.version == future-tag-without-v == 0.1.0-alpha.1`.
- [X] T003 Update `charts/mikebom-operator/templates/deployment.yaml` line 20 from `image: "{{ .Values.image.repository }}:{{ .Values.image.tag }}"` to `image: "{{ .Values.image.repository }}:{{ .Values.image.tag | default .Chart.AppVersion }}"` — chart's `appVersion` becomes the structural source of truth for which operator image gets pulled (research.md §6)
- [X] T004 Remove the hard-coded `tag: v0.1.0-alpha.1` line from `charts/mikebom-operator/values.yaml` so the deployment template's `default .Chart.AppVersion` takes effect; admins can still override via `--set image.tag=...` when they need to pin to a digest or a fork build

---

## Phase 2: Foundational (Rust integration test)

**Purpose**: Add the Rust integration test that catches in-repo version drift on every PR — well before any release attempt.

- [X] T005 Create new integration test file `crates/operator/tests/version_consistency.rs` that reads `Cargo.toml` workspace `version`, `charts/mikebom-operator/Chart.yaml` `version`, and `Chart.yaml` `appVersion`, then asserts all three equal (uses `std::fs` + workspace `serde_yaml` dep; ~50 lines per data-model.md). Runs in every `cargo test --workspace` invocation.

**Checkpoint**: After Phases 1–2, `cargo test --workspace` passes (the new test agrees that all three strings are `"0.1.0-alpha.1"`). User-story work can begin.

---

## Phase 3: User Story 3 — Version-stamping consistency check (Priority: P2; implemented FIRST as dependency)

**Story goal**: Pre-flight check in the release workflow fails-fast if the git tag, `Cargo.toml`, or `Chart.yaml` versions disagree — preventing any partial publish.

**Why landed early**: US1 (image signing) and US2 (chart signing) both add jobs that `needs: [versions]`. Landing US3's `versions` job first means US1/US2 can declare their dependency at impl time without dangling references. This story's P2 priority reflects "can ship without it if all four version strings happen to agree manually" — but production credibility demands the gate.

**Independent Test**: Modify `Chart.yaml` `version:` to a value that disagrees with `Cargo.toml`. Run `bash .github/scripts/check-versions.sh` (with `GITHUB_REF_NAME=v0.1.0-alpha.1` exported). Script MUST exit non-zero with a clear diff message naming the drifted file.

**Implementation:**

- [X] T006 [US3] Create new shell script `.github/scripts/check-versions.sh` (POSIX-compatible, ~40 lines). Inputs: `GITHUB_REF_NAME` env var. Logic: strip leading `v` from tag → parse `Cargo.toml` workspace `version` (grep+sed) → parse `Chart.yaml` `version` + `appVersion` (via `yq` installed by the workflow) → assert all four equal → exit non-zero with diff message on mismatch (research.md §5). `chmod +x` so the workflow can invoke directly.
- [X] T007 [US3] Update `.github/workflows/release.yml`: add a new `versions` job that runs BEFORE `image` and `chart`. Steps: checkout (SHA-pinned `actions/checkout@<sha>`), install `yq` (`pip install yq` or apt), invoke `.github/scripts/check-versions.sh`. Job permissions: `contents: read` only. Set `needs: [versions]` on the existing `image` job.
- [X] T008 [US3] In `.github/workflows/release.yml`, update the existing `chart` job's `needs:` to include `versions` transitively (already chains through `image`, but explicit `needs: [versions, image]` makes the dependency unambiguous).

---

## Phase 4: User Story 1 — Tagged release publishes signed image + SBOM attestation (Priority: P1) 🎯 MVP

**Story goal**: Operator container image at `ghcr.io/kusari-oss/mikebom-operator:<tag>` is cosign-signed by digest AND has a CycloneDX SBOM attached as a cosign attestation.

**Independent Test**: After a test tag push (e.g., `v0.1.0-alpha.1-test`), within 15 minutes: `cosign verify --certificate-identity-regexp '.*' --certificate-oidc-issuer-regexp '.*' ghcr.io/kusari-oss/mikebom-operator:v0.1.0-alpha.1-test` MUST return 0. `cosign verify-attestation --type cyclonedx ...` MUST also return 0.

**Implementation:**

- [X] T009 [US1] In `.github/workflows/release.yml` `image` job's `permissions:` block, add `id-token: write` (required for cosign keyless OIDC token exchange with Fulcio). Existing `contents: read` and `packages: write` stay.
- [X] T010 [US1] After the existing `docker/build-push-action` step in `release.yml`'s `image` job, add `uses: sigstore/cosign-installer@<40-hex-sha> # v3.X` (research.md §1). Pin to the latest v3.x SHA at time of implementation; trailing comment carries the version tag.
- [X] T010b [US1] **Closes M1 from /speckit-analyze**: add `id: build` to the existing `docker/build-push-action` step in `release.yml`'s `image` job (the existing step has NO `id:` today). Downstream cosign sign + attest steps (T011, T013) reference `${{ steps.build.outputs.digest }}` — without an `id:`, that expression resolves to empty and cosign signs the wrong reference or fails. This is the load-bearing precondition for all by-digest signing.
- [X] T011 [US1] After cosign installation, add a `run:` step that signs the image by DIGEST (not tag — see contract invariant 2): `cosign sign --yes ghcr.io/kusari-oss/mikebom-operator@${{ steps.build.outputs.digest }}`. T010b guarantees the `steps.build.outputs.digest` reference resolves correctly.
- [X] T012 [US1] Add `uses: anchore/sbom-action@<40-hex-sha> # v0.X` step that generates a CycloneDX JSON SBOM for the pushed image (research.md §2). Configure to scan by the image's full repo+digest, output to a file path the next step can read. SHA-pin with trailing version comment.
- [X] T013 [US1] Add a `run:` step `cosign attest --yes --predicate <sbom-path> --type cyclonedx ghcr.io/kusari-oss/mikebom-operator@${{ steps.build.outputs.digest }}` that attaches the SBOM as an attestation to the image's digest (research.md §3). Uses the same `steps.build.outputs.digest` reference pattern as T011 — both depend on T010b's `id: build` addition.

**Checkpoint**: After Phase 4, the MVP slice ships. A tagged release produces a cosign-signed image with an attached CycloneDX SBOM — verifiable by any user with the cosign CLI.

---

## Phase 5: User Story 2 — Tagged release publishes signed Helm chart (Priority: P1)

**Story goal**: Chart at `oci://ghcr.io/kusari-oss/charts/mikebom-operator:<tag-without-v>` is cosign-signed via the same keyless OIDC flow as the image. Chart's `version` AND `appVersion` match the tag (already enforced by US3's `versions` job).

**Independent Test**: After a tag push, `cosign verify --certificate-identity-regexp '.*' --certificate-oidc-issuer-regexp '.*' oci://ghcr.io/kusari-oss/charts/mikebom-operator:0.1.0-alpha.1-test` MUST return 0.

**Implementation:**

- [X] T014 [US2] In `.github/workflows/release.yml` `chart` job's `permissions:` block, add `id-token: write`. Existing `contents: read` and `packages: write` stay.
- [X] T015 [US2] After the existing `helm push` step in the `chart` job, add `uses: sigstore/cosign-installer@<40-hex-sha> # v3.X` (same SHA as T010 for symmetry).
- [X] T016 [US2] Add a `run:` step `cosign sign --yes oci://ghcr.io/kusari-oss/charts/mikebom-operator:${CHART_TAG}` (where `CHART_TAG` is the git tag minus `v`). Cosign signs the chart OCI artifact's digest under the hood (contract invariant 2).

**Checkpoint**: After Phase 5, both artifacts (image + chart) are signed. Constitution VII (Helm Chart Lockstep) is structurally complete — the `versions` job + chart template `Chart.AppVersion` default + both signatures = image and chart provably ship together at matching versions.

---

## Phase 6: User Story 4 — Automated GitHub Release with artifact links (Priority: P3)

**Story goal**: The workflow creates (or updates) a GitHub Release at the pushed tag with a body listing the image OCI ref, chart OCI ref, and cosign verification commands.

**Independent Test**: After a tag push, `gh release view v0.1.0-alpha.1-test` MUST return a release whose body contains the image OCI ref, chart OCI ref, and `cosign verify` + `cosign verify-attestation` commands the reader can paste.

**Implementation:**

- [X] T017 [US4] In `.github/workflows/release.yml`, add a new `release-notes` job with `needs: [image, chart]`, `permissions: { contents: write, id-token: read }`. Steps: checkout (SHA-pinned `actions/checkout`), then `uses: softprops/action-gh-release@<40-hex-sha> # v2.X` configured with `tag_name: ${{ github.ref_name }}`, `prerelease: ${{ contains(github.ref_name, '-') }}` (true for `-alpha.N`/`-rc.N` tags), and `body:` populated by a template (heredoc) that interpolates the tag into the artifact refs and verification commands. Idempotent for re-runs per FR-014 (research.md §4).

**Checkpoint**: After Phase 6, the full release surface is live. A maintainer's tag push produces: signed image + SBOM, signed chart, and a GitHub Release page admins can find from the repo.

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: Pre-PR gate, anti-regression, runbook, optional dry-run trigger.

- [X] T018 [P] Grep verification (FR-011 / SC-007): every `uses:` line in `.github/workflows/release.yml` MUST be SHA-pinned. Run `grep -E "uses: [^@]*@v[0-9]" .github/workflows/release.yml` and confirm **zero** matches. (Tag-based pinning is rejected; only `@<40-hex-sha> # <tag>` is acceptable.) Pinned with trailing `# vX` comment for human readability.
- [X] T019 [P] Grep verification (FR-015): top-level workflow has `permissions: contents: read` (default least-privilege), and every job that needs more has an explicit `permissions:` block. Run `grep -E "^permissions:" .github/workflows/release.yml` and inspect.
- [X] T020 [P] Grep verification (FR-012): the `Dockerfile` base image references remain pinned to manifest-list digests. Run `grep -E "^FROM [^@]*@sha256:" Dockerfile` and confirm ≥ 2 matches (one per build stage).
- [X] T021 [P] Create `docs/release-runbook.md` (NEW) per quickstart.md's maintainer-facing checklist: pre-tag steps, tag push, post-publish smoke (cosign verify, cosign verify-attestation, helm pull + install on kind), rollback procedure for partial publish. **Plus an explicit re-tag idempotency smoke section (closes M2 from /speckit-analyze, covers FR-014 + SC-006)**: after a successful publish, the maintainer SHOULD `git push --force origin <tag>` to re-trigger the workflow at the same commit, then verify (a) `gh release view <tag>` returns exactly ONE release (not duplicated), (b) `cosign verify ...` against the image still passes (re-sign is idempotent), (c) `cosign verify-attestation --type cyclonedx ...` still passes. Document the expected behavior and what to do if any of (a)/(b)/(c) regresses.
- [X] T022 Run pre-PR gate: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. The new `version_consistency` integration test MUST pass (all three version strings agree at `0.1.0-alpha.1`). All features 001–009 tests continue to pass.
- [X] T023 [P] **Required (was optional; closes M2 from /speckit-analyze as the automated counterpart to T021's runbook step)**: Add `workflow_dispatch:` trigger to `.github/workflows/release.yml` with a `dry_run: bool` input (default `true`). When `dry_run=true`, the workflow exercises all version checks, builds, SBOM generation, and cosign install — but skips the actual `docker push`, `helm push`, `cosign sign`, `cosign attest`, and `softprops/action-gh-release` steps (gated via `if: ${{ !inputs.dry_run }}` on each push-side step). Lets maintainers rehearse the release on any commit + verify the workflow is healthy before the real tag push.

---

## Dependencies

```
Phase 1 (Setup):
  T001 [Cargo.toml] ─┐
  T002 [Chart.yaml]  ├─▶ all four version strings agree
  T003 [deployment]  │
  T004 [values.yaml]─┘
  ↓
Phase 2 (Foundational):
  T005 [version_consistency.rs] — once committed alongside T001–T004,
                                   the new test passes on every PR
  ↓
Phase 3 (US3 Version pre-flight, P2 but landed first as dependency):
  T006 [check-versions.sh] → T007 [release.yml: versions job] → T008 [chart job needs versions]
  ↓
Phase 4 (US1 Image signing + SBOM, P1 MVP):
  T009 [permissions] → T010 [cosign-installer] → T010b [id: build on build-push] → T011 [cosign sign image via ${{ steps.build.outputs.digest }}] → T012 [sbom-action] → T013 [cosign attest via same digest]
  ↓
Phase 5 (US2 Chart signing, P1):
  T014 [permissions] → T015 [cosign-installer] → T016 [cosign sign chart]
  ↓
Phase 6 (US4 Release notes, P3):
  T017 [release-notes job with softprops/action-gh-release]
  ↓
Phase 7 (Polish):
  T018, T019, T020, T021, T023 [P] (independent checks/docs), T022 (sequential gate)
```

**Story independence**:
- US3 is landed FIRST despite its P2 priority because US1 and US2 declare `needs: versions` against the job US3 creates. The user-story-priority reflects business importance; the implementation order is dependency-driven.
- US1 and US2 are structurally similar (both add `cosign-installer` + `cosign sign` to their respective jobs). They could land in either order.
- US4 (Release notes) depends on US1+US2 having produced artifacts to link to.

## Parallel execution opportunities

- Phase 1 (T001–T004): 4 small file edits across 4 files — fully parallel.
- Phase 4 (T009–T013): sequential within the same `image` job's step list — file-level conflict.
- Phase 5 (T014–T016): sequential within the `chart` job — file-level conflict.
- Phase 7 (T018, T019, T020, T021, T023): 5 independent polish checks / new files — fully parallel.

## Implementation strategy

**MVP scope**: end of Phase 4 (T001–T013). After Phase 4 the operator image is published, signed, and SBOM-attested. The chart still ships (existing `release.yml`'s `helm push` step is preserved) but without cosign signature — admins can still pull and install, just without the chart sig verification path.

**Incremental delivery**:
- After Phase 4: signed image with SBOM is the v0.1.0-alpha.1 promise; chart works but is unsigned.
- After Phase 5: chart sig closes the constitution-VII loop.
- After Phase 6: GitHub Release page provides the canonical discovery surface.
- After Phase 7: pre-PR gate passes; runbook documented; ready for the actual `git tag v0.1.0-alpha.1 && git push` invocation.

**Test counts to expect** (cumulative, on top of features 001–009's 136 lib + 3 main.rs + 2 drift + 29 E2E):
- Phase 2 integration test: +1 (T005 `version_consistency`) → 137 lib tests total (counted as an integration test file run via `cargo test --workspace`).
- No new E2E tests for feature 010 (release infra is verified via the manual post-publish runbook smoke, not automated E2E).

## Format validation

All 24 tasks follow the format `- [ ] T### [P?] [Story?] Description with file path`. User-story phases (T006–T017, plus T010b inserted during the /speckit-analyze remediation pass) carry `[US1]`/`[US2]`/`[US3]`/`[US4]` labels. Setup, foundational, and polish phases carry no story label. Every task names ≥1 exact file path under `.github/`, `crates/`, `charts/`, `docs/`, `Dockerfile`, or `Cargo.toml`.
