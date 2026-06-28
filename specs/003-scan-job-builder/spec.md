# Feature Specification: scan-Job builder

**Feature Branch**: `003-scan-job-builder`

**Created**: 2026-06-27

**Status**: Draft

**Input**: User description: "Pure Job-spec builder: build_scan_job(NamespaceScanSpec, image_ref) -> batch/v1::Job constructing the 3-container Job (init-pull / mikebom-scan / output-upload) per the bootstrap plan §3, with unit tests on Job shape. No reconciler integration; Jobs not actually created from CRs yet. Smallest viable increment."

## Clarifications

### Session 2026-06-27

- Q: What shape should the v0.3 `output-upload` container take? → A: Chainguard busybox (digest-pinned, image picked during `/speckit-plan`) with command `["sh", "-c", "ls -la /workdir/out/ && cat /workdir/out/*.json"]`. Preserves the bootstrap plan §3 "3-container Job" contract, gives feature 004 contributors visible debug output via `kubectl logs` (directory listing + raw SBOM dump). Feature 004+ replaces image + command with concrete backend wiring; no other Job-shape changes needed.

## User Scenarios & Testing *(mandatory)*

### User Story 1 — Operator developer can produce a valid Job manifest (Priority: P1)

A contributor implementing feature 004+ (output backends) needs a single function that, given a `NamespaceScanSpec` and a container image ref, returns a complete `batch/v1.Job` manifest with the 3-container scan choreography wired up. They invoke that function in a unit test, serialize the output, and confirm it's a valid Kubernetes Job.

**Why this priority**: Without a single builder, every later feature (PVC/S3/OCI backends, reconciler integration, finalizers) would re-derive the Job shape with subtle divergence. P1 because this is the foundation every later feature builds on.

**Independent Test**: Call `build_scan_job(spec, image_ref)` in a unit test, serialize the returned struct to YAML, and assert the result is a complete `batch/v1.Job` manifest (correct `apiVersion`, `kind`, `metadata.name`, `spec.template.spec.containers` populated). Delivers value standalone — the function is usable by feature 004 the moment it merges.

**Acceptance Scenarios**:

1. **Given** a valid `NamespaceScanSpec` and a tagged image ref (`nginx:1.27.0`), **When** the contributor calls the builder, **Then** they receive a `batch/v1.Job` value with `apiVersion=batch/v1`, `kind=Job`, a deterministic DNS-1123-compliant `metadata.name`, and a populated pod template.
2. **Given** a digest-pinned image ref (`nginx@sha256:abc…`), **When** the contributor calls the builder, **Then** the resulting Job's `metadata.name` is still DNS-1123-compliant and the `mikebom-scan` container's args reference the digest-pinned ref unchanged.

---

### User Story 2 — Reviewer can verify the 3-container choreography (Priority: P2)

A security or architecture reviewer needs to confirm the Job matches the bootstrap plan §3's documented choreography: init-pull pulls the target image and extracts a rootfs; mikebom-scan runs `mikebom sbom scan --path …`; output-upload pushes the SBOM artifact. They read the Rust source (or the test assertions) and verify those three containers are present with the right shape.

**Why this priority**: This story protects against silent drift between the plan's documented design and the actual Job shape future contributors will see. Marked P2 because it's independently testable once US1 exists.

**Independent Test**: Run the unit-test suite. Assertions cover: exactly 3 containers in order `init-pull`, `mikebom-scan`, `output-upload`; all three mount the same `emptyDir` volume at `/workdir`; the `mikebom-scan` container uses `spec.mikebomImage` and invokes `mikebom sbom scan --path /workdir/rootfs --format <scanFormat> --output <scanFormat>=/workdir/out/…`; the `output-upload` container is present with the placeholder shape v0.3 ships (feature 004+ replaces its image and command).

**Acceptance Scenarios**:

1. **Given** a valid `NamespaceScanSpec`, **When** the unit tests run, **Then** every container-shape assertion passes (names, mount points, command/args structure).
2. **Given** `spec.scanFormat = spdx-3-json`, **When** the unit tests run, **Then** the `mikebom-scan` container's args include `--format spdx-3-json` and the output file path ends in the appropriate extension.

---

### User Story 3 — Job lifecycle policies prevent operational surprises (Priority: P3)

A cluster admin who reviews the chart's RBAC needs to know that scan Jobs auto-clean up, don't retry forever on failure, and run as one-shot pods. They read the Job's lifecycle fields (or the unit-test assertions) and confirm the defaults are operationally safe.

**Why this priority**: Cleanup and retry policy don't affect feature correctness in v0.3 (no Jobs are spawned yet), but they affect cluster hygiene the moment feature 004 wires the reconciler. P3 because it's a polish concern relative to US1 and US2.

**Independent Test**: Unit tests assert `restartPolicy: Never`, `completions: 1`, `parallelism: 1`, `backoffLimit ≤ 3`, and `ttlSecondsAfterFinished` set to a bounded value (≤ 1 hour).

**Acceptance Scenarios**:

1. **Given** a Job produced by the builder, **When** unit tests inspect the pod spec, **Then** `restartPolicy: Never`, `completions: 1`, `parallelism: 1`, `backoffLimit ≤ 3`.
2. **Given** a Job produced by the builder, **When** unit tests inspect `spec.ttlSecondsAfterFinished`, **Then** it is set to a value in `(0, 3600]` (auto-cleanup within an hour).

---

### Edge Cases

- Image ref is digest-pinned (`<repo>@sha256:<hex>`): builder accepts as-is; the Job's `metadata.name` derives a short SHA-256 prefix of the ref for uniqueness instead of trying to encode the digest inline.
- Image ref contains uppercase letters or `_`: builder sanitizes the visible portion of the Job name to DNS-1123 (lowercase + hyphens) while keeping the original ref in container args.
- `spec.scanFormat` is `cyclonedx-json`, `spdx-2.3-json`, or `spdx-3-json` (per feature 001's enum): the builder maps each to the correct `mikebom sbom scan --format` argument and the matching output-file extension (`.cdx.json`, `.spdx.json`, `.spdx3.json` or similar).
- Builder called with a `NamespaceScanSpec` whose `output.type` is `s3` or `oci` (feature 004+): the `output-upload` container's placeholder ignores the backend-specific config; future features replace the container's image and command rather than restructuring the Job.
- `spec.mikebomImage` is empty or unset: builder returns an error rather than producing a malformed Job — defense in depth even though feature 002's reconciler validation already catches this.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The builder MUST produce a `batch/v1.Job` value with correctly populated `apiVersion`, `kind`, and `metadata.name` fields. The name MUST be DNS-1123-compliant and ≤ 63 characters.
- **FR-002**: The Job's pod template MUST contain exactly three containers, in declaration order: `init-pull`, `mikebom-scan`, `output-upload`.
- **FR-003**: All three containers MUST mount a shared `emptyDir`-backed volume at the path `/workdir`. The volume MUST be defined in the Job's pod spec.
- **FR-004**: The `init-pull` container MUST use a configurable image reference (with a documented default) and run a command that produces a flat rootfs at `/workdir/rootfs` extracted from the target image's layers.
- **FR-005**: The `mikebom-scan` container MUST use `spec.mikebomImage` as its image and run `mikebom sbom scan --path /workdir/rootfs --format <scan-format> --output <scan-format>=/workdir/out/<sbom-file>` where `<scan-format>` derives from `spec.scanFormat`.
- **FR-006**: The `output-upload` container MUST be present in the Job's pod template using a digest-pinned Chainguard busybox image (the exact digest is resolved during `/speckit-plan`) and command `["sh", "-c", "ls -la /workdir/out/ && cat /workdir/out/*.json"]`. This is a v0.3 debug placeholder — it lists the SBOM directory and dumps the produced SBOM file(s) to stdout so `kubectl logs` shows whether the scan output landed. Feature 004+ replaces the image and command with concrete backend wiring; no other Job-shape changes required.
- **FR-007**: The Job spec MUST set `restartPolicy: Never`, `completions: 1`, `parallelism: 1`, and `backoffLimit ≤ 3`.
- **FR-008**: The Job spec MUST set `ttlSecondsAfterFinished` to a bounded value such that completed Jobs auto-cleanup within one hour.
- **FR-009**: The Job's `metadata.name` MUST be deterministic for the same `(NamespaceScan name, image ref)` pair — recomputing the builder against unchanged inputs produces the same name — and MUST include enough uniqueness to allow multiple images in the same NamespaceScan to be scanned concurrently without collisions.
- **FR-010**: The `mikebom-scan` container MUST declare resource requests appropriate for a one-shot scan (the specific values are a research decision in `/speckit-plan`; the FR is that requests are non-empty).
- **FR-011**: All container image references in the Job (init-pull, mikebom-scan, output-upload) MUST be tag- or digest-pinned. The builder MUST NOT emit `:latest` or unpinned refs.
- **FR-012**: When `spec.mikebomImage` is empty or whitespace, the builder MUST return an error rather than emit a malformed Job.

### Key Entities

- **NamespaceScanSpec**: Existing CRD spec from feature 001; consumed read-only by the builder.
- **Image ref**: Caller-supplied string in the form `<repo>:<tag>` or `<repo>@sha256:<hex>`. The builder doesn't validate the format beyond emptiness — it's the caller's responsibility to pass a sane ref.
- **Scan-Job manifest**: The `batch/v1.Job` value the builder produces. Consumed by feature 004+ (reconciler creates it; backend integration replaces the `output-upload` container).
- **Placeholder output-upload container**: A 3rd container that exists in the Job pod template with a stub image + command; documents the slot future backend features will fill.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The full unit-test suite for the builder runs in under 1 second on a developer laptop (pure-function, no I/O).
- **SC-002**: The Job manifest the builder produces passes Kubernetes server-side dry-run validation (`kubectl apply --dry-run=server`) when applied against any conformant Kubernetes 1.31+ cluster.
- **SC-003**: 100% of functional requirements (FR-001 through FR-012) have at least one corresponding assertion in the unit-test suite — verifiable by mapping FR IDs to test function names in `tasks.md`.
- **SC-004**: A future contributor wiring feature 004's PVC output backend needs to modify only the `output-upload` container's image and command — no changes to FR-002 through FR-005's surface.
- **SC-005**: A reviewer reading the unit-test suite can verify the 3-container choreography matches the bootstrap plan §3 without cross-referencing the Rust source — the test assertions name each container and its expected role.

## Assumptions

- **Image refs are trusted**: the builder doesn't validate registry domains or check for malicious patterns. Admission-control style validation is a future feature concern.
- **Default init-pull image**: a research decision in `/speckit-plan`; reasonable candidates include `cgr.dev/chainguard/skopeo` (Chainguard-distroless, security-leaning) and `quay.io/skopeo/stable`. Whichever is picked MUST be digest-pinned per FR-011.
- **`output-upload` v0.3 placeholder**: digest-pinned Chainguard busybox running `sh -c "ls -la /workdir/out/ && cat /workdir/out/*.json"` per Clarifications Q1 → C. Produces directory-listing + SBOM contents in container logs so feature 004 contributors can see whether scans actually emit files. Feature 004+ swaps the image and command for real backend wiring.
- **Job name derivation**: `nsscan-<sanitized-cr-name>-<short-image-hash>` where `<short-image-hash>` is a 7-character SHA-256 prefix of the image ref. Provides uniqueness without requiring DNS-1123 sanitization of the entire ref.
- **SBOM output file naming**: `<short-image-hash>.<format-extension>` — feature 003 doesn't actually pull image manifests to extract digests; the hash-prefix scheme works whether the input ref is tagged or digest-pinned.
- **Image-pull secrets**: assumed to be wired through the Job's `serviceAccountName`, which the reconciler will set in feature 004+. The builder doesn't take image-pull-secret args directly.
- **No multi-arch handling**: skopeo and modern container runtimes handle multi-arch refs transparently; the Job doesn't need explicit architecture hints in v0.3.

## Out of scope *(intentional deferral)*

- Reconciler creating Jobs (feature 002-skeleton + 004+ wire this).
- Enumerating pods in target namespaces to discover image refs (feature 007).
- Concrete PVC / S3 / OCI backend integration in the `output-upload` container (features 004 / 005 / 006).
- Job lifecycle watching: tracking Job status, updating `NamespaceScan.status.scannedImages` (feature 004+).
- Image-pull secret handling beyond `serviceAccountName` (later feature).
- Finalizers on `NamespaceScan` so deletion cleans up in-flight Jobs (feature 004+).
- Multi-arch image-specific Job customization (deferred indefinitely; runtime handles).
