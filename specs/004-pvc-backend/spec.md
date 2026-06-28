# Feature Specification: PVC output backend

**Feature Branch**: `004-pvc-backend`

**Created**: 2026-06-28

**Status**: Draft

**Input**: User description: "Builder-only PVC output backend: build_scan_job branches on spec.output.type=Pvc and produces a Job whose output-upload container copies SBOMs from /workdir/out/ to the user-supplied PVC. Helm values get an optional pvc section. Unit tests on the Job shape + kind dry-run E2E. No reconciler integration; Jobs still not actually spawned from CRs. Smallest viable increment matching feature 003's pattern."

## Clarifications

### Session 2026-06-28

- Q: Should `spec.output.pvc.pathPrefix` support templating with `{namespace}` / `{image-sha}` placeholders, or stay literal-only? → A: Literal only for v0.4 (Option A). Builder treats `pathPrefix` as a literal directory name; no substitution. Callers pre-template if they need per-namespace or per-image paths. Smallest builder surface; forward-compatible (future feature can add placeholders without breaking literal callers).

## User Scenarios & Testing *(mandatory)*

### User Story 1 — Builder produces a PVC-backed Job (Priority: P1)

A contributor implementing the reconciler integration (a future feature) calls `build_scan_job` with a `NamespaceScanSpec` whose `output.type == Pvc` and `output.pvc.claimName` set. They receive a `batch/v1.Job` whose pod template includes the user's PersistentVolumeClaim as a volume, and whose `output-upload` container mounts the PVC and copies SBOM files from `/workdir/out/` to a deterministic destination on the PVC.

**Why this priority**: This is the load-bearing change. Without it, the `output-upload` container stays the v0.3 placeholder and SBOMs never leave the Job pod. P1 because every later integration (reconciler, S3 backend, OCI backend) builds on this dispatch shape.

**Independent Test**: Unit-test the builder with a fixture `NamespaceScanSpec` whose `output.type = Pvc` and `output.pvc.claimName = "sbom-scratch"`. Assert: pod spec has a `persistentVolumeClaim` volume with the claim name; `output-upload` container mounts it at the configured mount path; container args copy `/workdir/out/*.json` to a path under the PVC mount. Delivers value standalone — feature 005 (S3) layers an additional dispatch arm; nothing in feature 004 needs to change.

**Acceptance Scenarios**:

1. **Given** a valid `NamespaceScanSpec` with `output.type = Pvc` and `output.pvc.claimName = "sbom-scratch"`, **When** the contributor calls the builder, **Then** the resulting Job has a pod spec with a `persistentVolumeClaim` volume referencing `sbom-scratch` AND the `output-upload` container has a volume mount targeting that PVC.
2. **Given** the same valid `NamespaceScanSpec` with `output.pvc.pathPrefix = "team-a"`, **When** the unit tests inspect the `output-upload` container's command, **Then** the command copies SBOMs to a destination under `<pvc-mount>/team-a/`.
3. **Given** a `NamespaceScanSpec` with `output.type = Pvc` but `output.pvc` is unset OR `output.pvc.claimName` is empty/whitespace, **When** the contributor calls the builder, **Then** the builder returns a new typed error (`MissingPvcConfig` or similar) rather than producing a malformed Job.

---

### User Story 2 — Cluster admin can configure the chart for PVC output (Priority: P2)

A cluster admin who installs the Helm chart needs to know how to wire a PersistentVolumeClaim into a `NamespaceScan` so the operator's Jobs actually have somewhere to write SBOMs. They read the chart's `values.yaml` and `docs/crd-reference.md` and see a worked example.

**Why this priority**: Without documentation, the PVC backend is invisible to anyone reading the chart. Documentation lands in this PR so the feature is operationally usable when the reconciler integration ships later.

**Independent Test**: Read `charts/mikebom-operator/values.yaml` and `docs/crd-reference.md` after this feature lands. Confirm both contain an explanatory example of `spec.output.type: pvc` with `claimName` and `pathPrefix` filled in, and a note that the operator does not create the PVC — the admin supplies it.

**Acceptance Scenarios**:

1. **Given** the merged feature, **When** an admin reads `docs/crd-reference.md`, **Then** they find an "Output backends" section with a worked PVC example and a sentence stating the operator does not create the PVC.
2. **Given** the merged feature, **When** an admin reads the chart's `values.yaml`, **Then** they find an example `mikebom.output` section showing how a Helm-installed default PVC name could be wired into a `NamespaceScan` template.

---

### User Story 3 — Existing builder tests stay green (Priority: P3)

A reviewer of this PR sees that feature 003's 16 unit tests + drift checks + e2e dry-run all still pass — feature 004's additions are strictly additive on the dispatch axis (Pvc vs Pvc-not-set), not modifications to the existing test surface.

**Why this priority**: Regression-prevention. Marked P3 because it's an emergent property; explicit verification just means running the existing test suite and observing no failures.

**Independent Test**: `cargo test --workspace` shows 22 lib tests (or more — feature 004 adds tests) pass; specifically, `pod_template_has_three_containers_in_correct_order`, `output_upload_is_v03_placeholder` (assertions may evolve to allow either v0.3 placeholder OR PVC variant), and `all_container_images_are_pinned` continue to pass.

**Acceptance Scenarios**:

1. **Given** the merged feature, **When** `cargo test --workspace` runs, **Then** all feature 001 + 002 + 003 tests pass alongside the new feature 004 tests with zero failures.
2. **Given** the merged feature, **When** a fixture spec has `output.type = Pvc`, **Then** the `output-upload` container shape differs from the v0.3 placeholder (real `cp` command instead of `ls && cat`).

---

### Edge Cases

- `spec.output.pvc.pathPrefix` is `None` or empty: SBOMs land at the root of the PVC mount (e.g., `<pvc-mount>/<sbom-file>`). No error.
- `spec.output.pvc.pathPrefix` contains a leading `/`: builder strips it before constructing the destination, treating the prefix as relative to the PVC mount.
- `spec.output.pvc.pathPrefix` contains shell metacharacters (`;`, `&`, `$`): builder treats them as literal characters; the resulting `cp` command will fail at runtime if the path is unsafe. The builder does not attempt to sanitize beyond the leading-slash strip.
- Builder called with `spec.output.type = S3` or `Oci`: feature 003's v0.3 placeholder output-upload container ships unchanged. Features 005/006 layer their own dispatch arms.
- PVC claim does not exist in the operator's namespace at Job-creation time: the Job pod will fail to start with `Unschedulable`/`Pending` — this is a runtime concern handled by Kubernetes; the builder does not check claim existence (it's a pure function).
- PVC is `ReadWriteOnce` and a second scan Job for the same `NamespaceScan` runs concurrently: pods will queue waiting for the volume. Cluster admin's responsibility to choose `ReadWriteMany` if concurrency is desired.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: When `spec.output.type == Pvc`, the builder MUST add a `Volume` to the Job's pod spec backed by `persistentVolumeClaim.claimName = spec.output.pvc.claimName`. The volume's name MUST be deterministic (e.g., `pvc-output`).
- **FR-002**: When `spec.output.type == Pvc`, the `output-upload` container MUST declare a `VolumeMount` referencing the PVC volume at the absolute path `/pvc-output`.
- **FR-003**: When `spec.output.type == Pvc`, the `output-upload` container's command MUST copy every SBOM file in `/workdir/out/` to a destination under the PVC mount path. If `spec.output.pvc.pathPrefix` is set, the destination MUST be `<pvc-mount>/<pathPrefix>/`; if unset or empty, the destination is `<pvc-mount>/`.
- **FR-004**: When `spec.output.type != Pvc`, the Job's pod spec MUST NOT include the PVC volume or any PVC volume mount. The `output-upload` container ships the v0.3 placeholder shape from feature 003 unchanged.
- **FR-005**: When `spec.output.type == Pvc` and `spec.output.pvc` is unset OR `spec.output.pvc.claimName.trim().is_empty()`, the builder MUST return a new typed error variant (`BuildScanJobError::MissingPvcConfig` or equivalent) rather than emit a malformed Job.
- **FR-006**: All container images in the Job (init-pull, mikebom-scan, output-upload) MUST remain tag- or digest-pinned (inheriting feature 003 FR-011); the PVC variant of output-upload MUST use the same digest-pinned distroless busybox image as the v0.3 placeholder.
- **FR-007**: The PVC volume MUST be mounted ONLY by the `output-upload` container — NOT by `init-pull` or `mikebom-scan`. This limits the blast radius if those containers misbehave (e.g., a compromised scan can't write to the user's PVC).
- **FR-008**: The PVC destination path inside the container MUST treat `spec.output.pvc.pathPrefix` as a literal directory name — no placeholder substitution. The builder strips a single leading `/` if present (so `"/team-a"` and `"team-a"` both produce `<pvc-mount>/team-a/`), and otherwise passes the prefix through verbatim.
- **FR-009**: The `output-upload` container's command MUST create the destination directory (`mkdir -p`) before copying, so a fresh PVC works without admin preparation.
- **FR-010**: The chart's `values.yaml` MUST include a commented-out example showing how to wire a default PVC name into Helm-managed `NamespaceScan` templates, and `docs/crd-reference.md` MUST gain an "Output backends" section with a worked PVC example.
- **FR-011**: The builder's error enum MUST be extensible — adding `MissingPvcConfig` (and future S3/OCI variants) MUST NOT require breaking changes to existing match arms in callers.
- **FR-012**: All feature 001 + 002 + 003 existing tests MUST continue to pass; this feature is strictly additive.

### Key Entities

- **NamespaceScanSpec.output**: Existing CRD field from feature 001. This feature consumes `output.type` and `output.pvc` read-only.
- **PVC Volume**: A `persistentVolumeClaim` volume added to the Job's pod spec when `output.type == Pvc`.
- **PVC mount**: A `VolumeMount` on the `output-upload` container at a known absolute path.
- **Destination path**: The directory inside the PVC mount where SBOMs land, derived from `output.pvc.pathPrefix` (subject to FR-008's clarification).
- **`BuildScanJobError::MissingPvcConfig`**: New variant added to the existing `BuildScanJobError` enum.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The full unit-test suite (existing + new feature 004 tests) runs in under 1 second on a developer laptop.
- **SC-002**: The Job manifest the builder produces for `output.type = Pvc` passes Kubernetes server-side dry-run validation (`kubectl apply --dry-run=server`) on a kind cluster, with the user's PVC referenced but not necessarily existing.
- **SC-003**: 100% of functional requirements (FR-001 through FR-012) have at least one corresponding assertion in the unit-test suite — verifiable by mapping FR IDs to test function names in `tasks.md`.
- **SC-004**: A future contributor wiring feature 005's S3 output backend only needs to add an `OutputType::S3` arm to the existing dispatch — no changes to the PVC code path.
- **SC-005**: A cluster admin reading the merged chart + docs can identify a working PVC backend configuration (claim name + path prefix + RWX-vs-RWO guidance) within 60 seconds.

## Assumptions

- **PVC pre-existence**: The PVC named in `spec.output.pvc.claimName` is assumed to already exist in the operator's namespace. The builder does not check; the reconciler (feature 002 today, with reconciler-creates-Jobs in a later feature) doesn't create PVCs.
- **PVC mount path**: hardcoded to `/pvc-output` in the `output-upload` container. Not currently configurable; if the v0.4 path conflicts with a user expectation, change is a follow-up feature.
- **Access mode**: cluster admin chooses RWX vs RWO based on whether the same NamespaceScan may have concurrent scans for different images. The builder is agnostic.
- **RBAC scope**: this feature does NOT add new RBAC rules. The Job pod uses the chart's existing ServiceAccount; `volumes[].persistentVolumeClaim` doesn't require pod-level RBAC.
- **`output-upload` container image**: stays digest-pinned Chainguard busybox (same as feature 003) — distroless, ships `sh + cp + mkdir`.
- **`pathPrefix` shell-safety**: builder does NOT sanitize beyond stripping a leading `/`. Cluster admins are responsible for not setting paths that break the destination shell command.

## Out of scope *(intentional deferral)*

- Reconciler integration (Jobs still not actually spawned from CRs in this feature — separate later milestone).
- S3 output backend (feature 005).
- OCI-registry output backend (feature 006).
- Operator-managed PVC creation (admin supplies the PVC).
- Image-pull secret handling on the PVC mount path.
- Advanced `pathPrefix` templating beyond what's resolved by FR-008's clarification.
- PVC pre-flight checks (existence, access mode validation) — runtime concern; not in scope.
- Multi-write concurrency on RWO PVCs — admin responsibility.
