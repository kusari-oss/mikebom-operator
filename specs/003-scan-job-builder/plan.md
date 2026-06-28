# Implementation Plan: scan-Job builder

**Branch**: `003-scan-job-builder` | **Date**: 2026-06-27 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/003-scan-job-builder/spec.md`

## Summary

Add a pure function `build_scan_job(&NamespaceScanSpec, &str) -> Result<batch::v1::Job, BuildScanJobError>` that constructs the 3-container scan Job (init-pull / mikebom-scan / output-upload) described in the bootstrap plan §3 and feature 003 spec. No reconciler integration: this feature ships the function + its unit tests + a kind dry-run E2E that validates the produced manifest against a real Kubernetes API server. Feature 004+ wires the reconciler to actually create these Jobs and replaces the placeholder `output-upload` container with concrete backend code.

## Technical Context

**Language/Version**: Rust 1.85+ stable (workspace toolchain, SHA-pinned in CI).

**Primary Dependencies**:
- `k8s-openapi` 0.23 (`v1_31` feature, already a workspace dep) — `batch::v1::Job`, `core::v1::{Container, EmptyDirVolumeSource, PodSpec, PodTemplateSpec, ResourceRequirements, Volume, VolumeMount}`, `apimachinery::pkg::api::resource::Quantity`.
- `kube` 0.97 — only for re-exporting `ResourceExt` and convenience traits if needed; the Job type itself comes from k8s-openapi.
- `thiserror` 2 (already a workspace dep) — typed `BuildScanJobError` enum.
- `sha2` 0.10 (**new workspace dep**) — SHA-256 short-prefix of the image ref for deterministic Job naming.
- `serde_yaml` 0.9 (already a workspace dep, used by feature 001's CRD generator) — for serializing the produced Job to YAML in the dry-run E2E.

**Storage**: N/A — pure data transformation.

**Testing**:
- Unit tests in `crates/operator/src/scan_job/mod.rs` (or `scan_job.rs`) using a `#[cfg(test)] mod tests` block. Pure-function tests with no I/O — meets SC-001's <1s budget trivially.
- New kind-based E2E `e2e/tests/scan_job_dryrun.rs` (gated by `MIKEBOM_OPERATOR_E2E=1`) that calls the builder, serializes the Job to YAML, and asserts `kubectl apply --dry-run=server` accepts it. Satisfies constitution VI (the principle explicitly names "Job-template construction" as triggering the rule); dry-run avoids the cost of actually scheduling the pod.

**Target Platform**: Linux x86_64 production / macOS dev — pure Rust.

**Project Type**: Rust workspace; new module inside the `operator` crate.

**Performance Goals**: Builder runs in microseconds. SC-001 (full unit-test suite < 1s) easy to satisfy — no I/O, just struct construction + SHA-256 hashing.

**Constraints**:
- Job `metadata.name` MUST be DNS-1123-compliant + ≤ 63 chars + deterministic (FR-001, FR-009).
- All container image refs MUST be tag- or digest-pinned (FR-011); we pin to digests with a `# vX.Y.Z` comment for human readability.
- Three-container choreography MUST match the bootstrap plan §3 (FR-002).
- Builder MUST return `Err` for empty `spec.mikebomImage` rather than emit a malformed Job (FR-012).

**Scale/Scope**: One builder invocation per `(NamespaceScan, image_ref)` pair. v0.x expects ≤ 100 Jobs per NamespaceScan; builder cost is negligible at that scale.

## Constitution Check

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Pure Rust where reasonable | PASS | `sha2` and `serde_yaml` are pure-Rust; no new C transitives |
| II. USE not EMBED | PASS | `mikebom-scan` container uses `spec.mikebomImage` — exactly the USE pattern |
| III. Fail Closed on RBAC | N/A | Builder doesn't touch the K8s API |
| IV. CRD Backward Compatibility | N/A | No CRD shape changes in this feature |
| V. SBOM-Format Agnostic | PASS | Builder accepts CDX 1.6 / SPDX 2.3 / SPDX 3 via `spec.scanFormat` and maps each to the right `mikebom sbom scan --format` arg; never parses SBOM contents |
| VI. Hermetic E2E Tests | PASS | Adds `e2e/tests/scan_job_dryrun.rs` — Job-template construction is explicitly named in the principle; dry-run E2E satisfies it without scheduling actual pods |
| VII. Helm Chart Lockstep | N/A | No CRD or chart schema changes |

All gates pass. No `## Complexity Tracking` section needed.

## Project Structure

### Documentation (this feature)

```text
specs/003-scan-job-builder/
├── plan.md              # this file
├── research.md          # Phase 0 (8 decisions: init-pull image, output-upload digest, layer-flatten step, mikebom-scan resources, Job-name scheme, YAML serialization, dry-run E2E shape, error-enum design)
├── data-model.md        # Phase 1: ScanJobConfig + Job-manifest shape + state-transition n/a
├── quickstart.md        # Phase 1: how to call the builder, inspect output, extend for feature 004+
├── contracts/
│   └── build-scan-job.md   # Function signature + Job-shape stability contract
└── tasks.md             # /speckit-tasks output (not created here)
```

### Source Code (repository root)

```text
Cargo.toml                            # add `sha2 = "0.10"` to [workspace.dependencies]

crates/operator/
├── Cargo.toml                        # add `sha2.workspace = true` to [dependencies]
├── src/
│   ├── lib.rs                        # add `pub mod scan_job;`
│   └── scan_job/
│       ├── mod.rs                    # NEW: `build_scan_job` + `BuildScanJobError` + image defaults + #[cfg(test)] unit tests
│       └── (potentially split into containers.rs + naming.rs if mod.rs gets too long)

e2e/tests/
└── scan_job_dryrun.rs                # NEW: gated kind E2E running `kubectl apply --dry-run=server` on the builder output

docs/
└── architecture.md                   # ADD note pointing at the new builder module in the Reconciler subsection
```

**Structure Decision**:

A new top-level `crates/operator/src/scan_job/` module rather than a single `scan_job.rs` file — the spec implies the file will grow as feature 004+ adds backend-specific output container construction. Starting as a module makes it easy to split into `containers.rs` (container builders), `naming.rs` (Job-name derivation), and `defaults.rs` (digest-pinned image constants) without churning import paths later. For v0.3 with ~200 lines, everything lives in `mod.rs`; the split lands when the file crosses ~400 lines (a typical Rust readability threshold).

Image defaults (init-pull, output-upload) are compile-time `const` strings with digests resolved at plan time per research §R1 / §R2 — checked into source rather than env-overridden, so the kind dry-run E2E doesn't need configuration. The compile-time pinning matches feature 001's bootstrap-amendment commitment to digest-pinned base images.
