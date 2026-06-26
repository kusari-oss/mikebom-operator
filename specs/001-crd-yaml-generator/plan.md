# Implementation Plan: CRD YAML generator + drift check

**Branch**: `001-crd-yaml-generator` | **Date**: 2026-06-26 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/001-crd-yaml-generator/spec.md`

## Summary

Add a `mikebom-operator-ctl crd` subcommand that emits the `NamespaceScan` CRD YAML programmatically from the kube `CustomResource` derive at `crates/operator/src/crds/namespace_scan.rs`, and a `cargo test` integration test in the operator crate that asserts the checked-in chart YAML at `charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml` matches the generator output byte-for-byte. The chart's CRD file is currently a placeholder comment, and constitution principle VII (Helm Chart Lockstep) is unenforced — this feature lands both halves at once.

## Technical Context

**Language/Version**: Rust 1.85+ (workspace toolchain via `dtolnay/rust-toolchain@stable` in CI)

**Primary Dependencies**: kube 0.97 (`CustomResource` derive + `CustomResourceExt::crd()`), k8s-openapi 0.23 (feature `v1_31`, provides `CustomResourceDefinition`), schemars 0.8 (`JsonSchema` derive — bridges Rust types to OpenAPI schemas), serde_yaml 0.9 (YAML serialization), clap 4 (ctl subcommand parsing). Added: pretty_assertions 1 (dev-dep, for clearer diff output in `crd_drift.rs`).

**Storage**: N/A — this is build/test tooling.

**Testing**: `cargo test --workspace` runs the drift check as an integration test in `crates/operator/tests/crd_drift.rs`. Kind-based E2E in `e2e/tests/crd_install.rs` (gated behind `MIKEBOM_OPERATOR_E2E=1`) satisfies constitution VI for the CRD-shape change.

**Target Platform**: Linux x86_64 (CI); macOS (dev). Windows untested.

**Project Type**: Rust workspace — multi-crate (operator lib + bin, ctl bin, e2e test crate).

**Performance Goals**: `mikebom-operator-ctl crd` runs in <1s wall-clock. `cargo test --workspace` drift check adds <2s to the existing suite. Kind-E2E CRD install asserts within 60s of `helm install`.

**Constraints**: Byte-identical generator output across runs (deterministic key order, no embedded timestamps, no random IDs). Chart YAML and Rust struct cannot drift past a single PR.

**Scale/Scope**: 1 CRD in v0.1 (`NamespaceScan`). Designed for non-breaking extension to N CRDs in v0.2+.

## Constitution Check

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Pure Rust where reasonable | PASS | No new C deps; everything Rust-native |
| II. USE not EMBED | PASS | No static linking of mikebom; this is operator-internal tooling |
| III. Fail Closed on RBAC | N/A | Feature has no runtime RBAC surface |
| IV. CRD Backward Compatibility | PASS | FR-008 mandates non-breaking subcommand-extension contract |
| V. SBOM-Format Agnostic | N/A | Feature touches CRD shape, not SBOM ingestion |
| VI. Hermetic E2E Tests | PASS | Adds gated `e2e/tests/crd_install.rs` — CRD shape touch falls under VI's scope |
| VII. Helm Chart Lockstep | PASS | This feature IS principle VII's enforcement mechanism |

All gates pass. No violations — `## Complexity Tracking` section omitted.

## Project Structure

### Documentation (this feature)

```text
specs/001-crd-yaml-generator/
├── plan.md              # this file
├── research.md          # Phase 0 decisions (serializer, determinism, structure)
├── data-model.md        # Phase 1: Rust types ↔ generated YAML shape
├── quickstart.md        # Phase 1: contributor regen workflow
├── contracts/
│   └── cli.md           # mikebom-operator-ctl crd CLI contract
└── tasks.md             # /speckit-tasks output (not created here)
```

### Source Code (repository root)

```text
crates/operator/
├── Cargo.toml                   # add pretty_assertions dev-dep
├── src/
│   ├── lib.rs                   # NEW: pub use of crds, output, reconcile, status
│   ├── main.rs                  # unchanged behavior; now uses operator::… via lib
│   └── crds/
│       ├── mod.rs               # add `pub mod serialize;`
│       ├── namespace_scan.rs    # unchanged (struct already correct)
│       └── serialize.rs         # NEW: pub fn crd_yaml<K: CustomResourceExt>() -> String
└── tests/
    └── crd_drift.rs             # NEW: asserts include_str!(chart) == crd_yaml::<NamespaceScan>()
                                 # plus a determinism sub-test (two runs == byte-equal)

crates/ctl/
├── Cargo.toml                   # add `operator` workspace dep
└── src/main.rs                  # replace placeholder with real `crd` impl calling
                                 # operator::crds::serialize::crd_yaml::<NamespaceScan>()

charts/mikebom-operator/
└── crds/
    └── namespacescan.kusari.dev_v1.yaml   # REPLACE placeholder with generator output (FR-009)

e2e/
├── Cargo.toml                   # unchanged
└── tests/
    └── crd_install.rs           # NEW: gated kind-E2E asserting `helm install` registers CRD

docs/
└── crd-reference.md             # add the regen command + drift-check workflow
```

**Structure Decision**: The operator crate transitions from bin-only to lib + bin. Rationale: the `ctl` binary AND the operator's `tests/crd_drift.rs` must serialize the CRD through **the same** code path — otherwise FR-002 ("no hand-written YAML intermediate") and FR-004 (byte-identical output) are both at risk if two parallel serializers exist. A single `operator::crds::serialize::crd_yaml::<K>()` function with one caller in each consumer is the smallest possible change that satisfies both requirements.
