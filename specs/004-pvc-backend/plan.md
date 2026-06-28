# Implementation Plan: PVC output backend

**Branch**: `004-pvc-backend` | **Date**: 2026-06-28 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/004-pvc-backend/spec.md`

## Summary

Extend feature 003's `build_scan_job` to dispatch on `spec.output.type`. When `Pvc`, the builder adds a `persistentVolumeClaim` volume to the Job's pod spec, mounts it only on the `output-upload` container (blast-radius limit per FR-007), and rewrites the container's command from feature 003's placeholder (`ls && cat`) to a real `mkdir -p <dest> && cp /workdir/out/*.json <dest>/` against the PVC mount. A new `BuildScanJobError::MissingPvcConfig` variant catches malformed specs. No reconciler integration; feature 004 ships builder + chart docs + unit tests + a kind dry-run E2E that exercises the PVC manifest path. Features 005 (S3) and 006 (OCI) layer additional dispatch arms in the same shape.

## Technical Context

**Language/Version**: Rust 1.85+ stable (workspace toolchain).

**Primary Dependencies**:
- `k8s-openapi` 0.23 — `PersistentVolumeClaimVolumeSource` (existing workspace dep, no new feature flags needed).
- `kube`, `thiserror`, `sha2`, `serde_yaml` — all already present; this feature adds no new crates.

**Storage**: N/A — pure data transformation, same as feature 003.

**Testing**:
- Unit tests in `crates/operator/src/scan_job/mod.rs` (extending the existing `#[cfg(test)] mod tests`) — pure-function tests, SC-001's <1s budget trivially satisfied.
- Extend `e2e/tests/scan_job_dryrun.rs` (gated by `MIKEBOM_OPERATOR_E2E=1`) with a PVC-variant fixture that produces a Job whose YAML passes `kubectl apply --dry-run=server`. Satisfies constitution VI for the Job-template-construction touch.

**Target Platform**: Linux x86_64 / macOS dev — same as feature 003.

**Project Type**: Rust workspace — implementation lives in the existing `operator` crate's `scan_job` module.

**Performance Goals**: Same as feature 003. Builder runs in microseconds; full unit-test suite <1s; dry-run E2E <5s per format-PVC variant pair.

**Constraints**:
- Existing feature 001/002/003 tests MUST continue to pass (spec FR-012; this is a strictly additive feature).
- PVC volume MUST be mounted ONLY by `output-upload` (FR-007 — blast-radius limit; init-pull / mikebom-scan continue to mount only `workdir`).
- `BuildScanJobError::MissingPvcConfig` variant addition does not break match arms because the enum is `#[non_exhaustive]` (feature 003 R8).
- Container image references stay digest-pinned (FR-006); no image changes from feature 003.

**Scale/Scope**: Same as feature 003. One builder invocation per `(NamespaceScan, image_ref)`; the dispatch on `output.type` is a single branch.

## Constitution Check

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Pure Rust where reasonable | PASS | No new deps; existing pure-Rust stack |
| II. USE not EMBED | PASS | `mikebom-scan` container still consumes `spec.mikebomImage` unchanged |
| III. Fail Closed on RBAC | N/A | No runtime RBAC surface; chart RBAC unchanged |
| IV. CRD Backward Compatibility | PASS | Uses existing `output.pvc` field from feature 001; no CRD shape changes |
| V. SBOM-Format Agnostic | PASS | `mikebom-scan` container's args unchanged; format dispatch lives in feature 003's code |
| VI. Hermetic E2E Tests | PASS | Extends `e2e/tests/scan_job_dryrun.rs` with PVC variant — Job-template touch satisfied |
| VII. Helm Chart Lockstep | N/A | No CRD shape changes; chart `values.yaml` + docs touches don't affect the drift check |

All gates pass. No `## Complexity Tracking` section needed.

## Project Structure

### Documentation (this feature)

```text
specs/004-pvc-backend/
├── plan.md                          # this file
├── research.md                      # Phase 0 (8 decisions: PVC volume construction, cp+mkdir command shape, leading-slash strip, MissingPvcConfig design, test fixture pattern, kind dry-run E2E shape, chart values example shape, output_upload_is_v03_placeholder test rename)
├── data-model.md                    # Phase 1: dispatch shape + new error variant + FR→test mapping
├── quickstart.md                    # Phase 1: admin install workflow + contributor extension notes
├── contracts/
│   └── output-backends.md           # Dispatch contract (PVC arm now; S3/OCI arms reserved)
└── tasks.md                         # /speckit-tasks output (not created here)
```

### Source Code (repository root)

```text
crates/operator/src/scan_job/
├── mod.rs                           # MODIFY:
│                                    #   - Refactor build_output_upload_container() into a dispatch fn taking &Output
│                                    #   - Add `output-upload`-builder helper variants (placeholder + pvc)
│                                    #   - Add MissingPvcConfig variant to BuildScanJobError
│                                    #   - Add PVC volume to JobSpec's pod template when output.type == Pvc
│                                    #   - New tests for PVC dispatch + MissingPvcConfig + non-PVC unchanged
│                                    #   - Rename output_upload_is_v03_placeholder → output_upload_non_pvc_is_v03_placeholder
│                                    # No new sibling files in this feature; if mod.rs grows past ~700 lines,
│                                    # split into containers.rs + naming.rs + defaults.rs per feature 003's
│                                    # plan §"Structure Decision" — but probably not needed at v0.4 size.

e2e/tests/
└── scan_job_dryrun.rs               # MODIFY: add PVC-variant fixture + dry-run assertion
                                     # (adds a new #[test] fn for the PVC path)

charts/mikebom-operator/
└── values.yaml                      # MODIFY: add commented `mikebom.output` example showing PVC backend wiring

docs/
└── crd-reference.md                 # MODIFY: add "Output backends" section with PVC example
```

**Structure Decision**:

Everything lands in the existing `scan_job::mod.rs`. The dispatch is small (one match on `output.type` returning the appropriate `Container`), and the new tests live alongside feature 003's. No new sibling files yet — the file growth from ~470 to ~600 lines stays under the ~700-line readability threshold I called out in feature 003's plan.

The `output-backends.md` contract is new; it owns the dispatch shape so features 005/006 can extend the same contract without rewriting it. Feature 003's `build-scan-job.md` contract stays authoritative for the broader function signature; `output-backends.md` is a subcontract for the `output-upload` container's variation.
