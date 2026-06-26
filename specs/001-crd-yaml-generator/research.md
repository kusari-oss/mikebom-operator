# Phase 0 — Research

This phase resolves the open technical questions implied by the spec's FRs. Each decision below is a binding input to Phase 1 design.

## R1: YAML serializer for `kube::CustomResourceExt::crd()` output

**Decision**: Use `serde_yaml::to_string(&K::crd())` with no canonicalization step.

**Rationale**:
- `kube::CustomResourceExt::crd()` (auto-derived by `#[derive(CustomResource)]`) returns a `k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition`.
- That type's `serde` derives use stable Rust struct-field order at every level. Nested property maps use `BTreeMap` (alphabetical) rather than `HashMap`.
- `serde_yaml 0.9::to_string` is deterministic for any input with stable iteration order.
- The combined pipeline (kube derive → k8s-openapi types → serde_yaml) produces byte-identical output across repeated runs of the same struct. R2 enforces this with a test.

**Alternatives considered**:
- `serde_yml` (community fork of the abandoned serde_yaml). Same API, no behavioral difference for our case. Defer until serde_yaml shows actual problems (unmaintained ≠ broken).
- Custom canonicalization (e.g., round-trip through JSON, sort all keys). Adds a dep and a serialization step with no observable benefit. Rejected.
- Hand-rolled `BTreeMap`-based emitter. Violates FR-002 (would diverge from the kube derive output). Rejected.

## R2: Determinism enforcement

**Decision**: Add a `determinism` sub-test inside `crd_drift.rs` that calls `crd_yaml::<NamespaceScan>()` twice and asserts byte-equality.

**Rationale**: Cheap (two function calls + `assert_eq!`) and catches a class of regression (CI drift-check flapping) that would otherwise erode trust in the primary check. If kube-rs or serde_yaml ever introduces nondeterminism (HashMap-backed map type at a new layer, timestamp embedding, etc.), this test fires immediately with a clear message rather than producing intermittent CI failures.

## R3: Where the shared serializer lives

**Decision**: `crates/operator/src/crds/serialize.rs`, exposed via a new `crates/operator/src/lib.rs`. The `ctl` binary depends on `operator` as a workspace path dep; the integration test in `crates/operator/tests/crd_drift.rs` consumes it via the standard cargo-test pattern.

**Rationale**: One function, two consumers → one source of truth. Avoids the failure mode where `ctl` and the test serialize the same struct two different ways and the drift check passes against itself but the chart YAML reflects a third serialization. Converting `operator` from bin-only to lib + bin is a Cargo convention that doesn't require any Cargo.toml shape change beyond adding `src/lib.rs` (Cargo auto-detects).

**Alternatives considered**:
- Inline serialization in BOTH `ctl/src/main.rs` AND `tests/crd_drift.rs`. Violates the single-source-of-truth principle and risks silent divergence. Rejected.
- New `mikebom-operator-common` crate. Over-decomposed for one function (~30 lines). Rejected.

## R4: CI integration shape

**Decision**: The integration test runs as part of the existing `cargo test --workspace` step in `.github/workflows/ci.yml`. No new CI workflow file, no new step.

**Rationale**: Resolved in spec Clarifications Q1 → A. The existing CI workflow already runs `cargo test --workspace`; adding the drift check as `crates/operator/tests/crd_drift.rs` makes it automatic without touching workflow files.

## R5: Constitution VI satisfaction (kind-based E2E)

**Decision**: Add `e2e/tests/crd_install.rs` gated behind `MIKEBOM_OPERATOR_E2E=1`. The test:
1. Spins up (or reuses) the kind cluster from `e2e/kind-cluster.yaml`.
2. Runs `helm install mikebom-operator charts/mikebom-operator/ -n kusari-operator --create-namespace`.
3. Asserts `kubectl get crd namespacescans.kusari.dev` returns a CRD with `spec.group=kusari.dev` and `spec.versions[0].name=v1alpha1`.
4. Cleans up (Helm uninstall + namespace delete).

No reconciler exercise; that's feature 008's scope.

**Rationale**: Constitution VI is non-negotiable and explicitly lists "CRD shape" as a triggering touch. The minimal CRD-install E2E satisfies the principle without inheriting the complexity of the full kind harness. When feature 008 lands its broader harness, this test can be folded in or kept as a fast-path smoke.

## R6: Subcommand argument shape

**Decision**: `mikebom-operator-ctl crd` with no positional argument. Internal implementation calls `crd_yaml::<NamespaceScan>()` directly. Optional flag: `--output <PATH>` writes to a file instead of stdout.

**Rationale**: Resolved in spec Clarifications Q2 → A. `clap`'s derive macro handles "subcommand with no args + optional flag" trivially.

## R7: Replacing the chart placeholder

**Decision**: The first commit in the implementation phase runs the generator and overwrites `charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml` with real generated content. Otherwise the integration test would fail on first run.

**Rationale**: FR-009. Order matters: build the lib + ctl wiring first, then run the generator and commit the output, then add the integration test. (Or build everything together and run `mikebom-operator-ctl crd --output ...` as a one-shot regen.)

## R8: include_str! path stability

**Decision**: Use `include_str!("../../../charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml")` from `crates/operator/tests/crd_drift.rs`.

**Rationale**: The path is relative to the source file. `crates/operator/tests/` → `..` (operator) → `..` (crates) → `..` (repo root) → `charts/...`. Three levels up. The chart YAML path is stable per the layout decision in plan §"Repo layout" (frozen by the bootstrap PR).

**Alternative considered**: `env!("CARGO_MANIFEST_DIR")` + `std::fs::read_to_string` for path-resolution-agnostic loading. Heavier; only justified if the chart path becomes parameterized later. Rejected for v0.1.
