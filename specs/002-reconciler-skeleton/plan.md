# Implementation Plan: NamespaceScan reconciler skeleton

**Branch**: `002-reconciler-skeleton` | **Date**: 2026-06-27 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/002-reconciler-skeleton/spec.md`

## Summary

Wire the kube-rs `Controller` for `NamespaceScan` CRs, set up leader election via the `coordination.k8s.io/v1.Lease` API, and implement a reconcile function that acknowledges every CR by setting `Ready=False` with `reason=NotYetReconciled` (or `InvalidSpec` for malformed targets) and updating a new `status.lastReconciledAt` timestamp on every reconcile cycle. No pod enumeration, no Job creation — those are features 003 and 007. This feature converts the operator from "scaffold that compiles but does nothing" to "scaffold that runs in-cluster, observes CRs, and reports its presence to humans and tooling."

## Technical Context

**Language/Version**: Rust 1.85+ stable (workspace toolchain via `dtolnay/rust-toolchain@stable` SHA-pinned in CI).

**Primary Dependencies**:
- `kube` 0.97 — `Controller` from `kube::runtime`, leader-election helpers, `Client`.
- `k8s-openapi` 0.23 (feature `v1_31`) — provides `coordination.k8s.io.Lease` type alongside the existing `NamespaceScan` derive surface.
- `tokio` 1 (`rt-multi-thread`, `macros`, `signal`, `time`) — async runtime; the existing workspace dep is already configured.
- `tracing-subscriber` 0.3 — **add the `json` feature** to the workspace dep so logs serialize as JSON per FR-009 / SC-005.
- `chrono` 0.4 (`serde` feature, already pinned) — RFC 3339 timestamps for `lastReconciledAt`.
- `thiserror` 2 (already in workspace deps) — typed reconciler error enum.
- `futures` 0.3 (already in workspace deps) — `StreamExt` for Controller stream.

**Storage**: Kubernetes API. The operator reads `NamespaceScan` CRs cluster-wide and writes their `status` subresource. The leader-election `Lease` lives in the operator's namespace (`POD_NAMESPACE` env var; defaults to `kusari-operator` per the Helm chart). No external storage.

**Testing**:
- `cargo test --workspace` for unit tests (the reconcile decision-making is unit-testable in principle: given a CR shape, what status conditions should be written?) and feature 001's drift check (which will fire if the chart CRD YAML isn't regenerated after adding `lastReconciledAt`).
- New kind-based E2E in `e2e/tests/reconciler_skeleton.rs` (gated by `MIKEBOM_OPERATOR_E2E=1`) covering US1 + US2. US3 (leader-election failover) is observable in the same E2E but exercised optionally because killing pods slows the test.

**Target Platform**: Linux x86_64 (container runtime, CI); macOS for local dev. Containers built from the SHA-pinned `gcr.io/distroless/cc-debian12:nonroot` base from feature 001's Dockerfile.

**Project Type**: Rust workspace — implementation lives in the `operator` crate (lib + bin).

**Performance Goals**:
- helm install → operator pod `Ready` < 30s (SC-001).
- `kubectl apply -f namespacescan.yaml` → `status.lastReconciledAt` populated < 10s (SC-002).
- Leader pod killed → new leader acquires + resumes reconcile < 30s (SC-003).
- 24h continuous run with no OOM kills, leader flapping < 1 transition/hour under steady state, bounded log volume (SC-004).

**Constraints**:
- Status updates MUST be idempotent (FR-010) — reconciling an unchanged CR yields the same conditions; only `lastReconciledAt` refreshes.
- Structured JSON logs ONLY (FR-009, SC-005) — `tracing-subscriber::fmt().json()` initializer; no plain-text fallback even in dev.
- Operator MUST NOT create `batch/v1.Job`s (FR-005) and MUST NOT enumerate `Pod`s (FR-006) — verified by inspecting the implementation surface; no `kube::Api::<Job>::create` calls exist anywhere in this feature.
- Leader election Lease MUST be visible to `kubectl get lease -n kusari-operator` (FR-007).

**Scale/Scope**: 1–100 `NamespaceScan` CRs per cluster expected for v0.x. Operator default = 1 replica per the Helm chart; HA scales to 2+ with leader-election ensuring single active reconciler.

## Constitution Check

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Pure Rust where reasonable | PASS | All new code is Rust; no new C transitives introduced |
| II. USE not EMBED | N/A | No mikebom-cli coupling in the reconciler skeleton |
| III. Fail Closed on RBAC | N/A | This feature doesn't enumerate any narrowable scope; principle III applies in feature 003+ when target namespaces are actually read |
| IV. CRD Backward Compatibility | PASS | New `lastReconciledAt` is an additive optional field on `v1alpha1`; no `v1alpha2` migration required |
| V. SBOM-Format Agnostic | N/A | No SBOM handling |
| VI. Hermetic E2E Tests | PASS | Adds `e2e/tests/reconciler_skeleton.rs` exercising US1 + US2 against a kind cluster |
| VII. Helm Chart Lockstep | PASS | CRD shape change auto-detected by feature 001's `crd_drift.rs` if the chart YAML isn't regenerated; tasks include regen + commit |

All gates pass. No `## Complexity Tracking` section needed.

## Project Structure

### Documentation (this feature)

```text
specs/002-reconciler-skeleton/
├── plan.md                          # this file
├── research.md                      # Phase 0 (8 decisions: leader-election crate path, log format, status patch strategy, requeue cadence, validation locus, kind-E2E shape, RBAC verification, lastReconciledAt serialization)
├── data-model.md                    # Phase 1: NamespaceScanStatus addition + condition vocabulary + Lease shape
├── quickstart.md                    # Phase 1: cluster-admin + contributor + debug workflows
├── contracts/
│   ├── namespacescan-status.md      # status condition + lastReconciledAt user-facing contract
│   └── leader-election.md           # Lease.holderIdentity convention
└── tasks.md                         # /speckit-tasks output (not created here)
```

### Source Code (repository root)

```text
crates/operator/
├── Cargo.toml                       # unchanged (workspace deps handle tracing-subscriber feature flag)
├── src/
│   ├── lib.rs                       # unchanged (modules already pub)
│   ├── main.rs                      # WIRE: tracing JSON init, kube::Client::try_default, leader::run_with_leadership, Controller::new(...).run(...)
│   ├── crds/
│   │   └── namespace_scan.rs        # ADD: `last_reconciled_at: Option<String>` to NamespaceScanStatus
│   ├── reconcile/
│   │   └── namespace_scan.rs        # IMPL: reconcile(obj, ctx) -> Result<Action, Error>; error_policy; Ctx struct
│   ├── status.rs                    # IMPL: `set_ready_condition(status, reason, message)` and `touch_last_reconciled_at(status)`; centralizes condition shape
│   └── leader.rs                    # NEW: `run_with_leadership(...)` wraps a closure in leader-election against coordination.k8s.io Lease
└── tests/
    └── crd_drift.rs                 # unchanged; chart YAML regen makes it stay green

workspace
└── Cargo.toml                       # ADD `json` feature to tracing-subscriber workspace dep

charts/mikebom-operator/
├── crds/
│   └── namespacescan.kusari.dev_v1.yaml   # REGEN via `cargo run --bin mikebom-operator-ctl -- crd --output ...` (task)
└── values.yaml                      # unchanged

e2e/tests/
└── reconciler_skeleton.rs           # NEW: gated kind E2E covering US1 + US2 (and a fast-path US3 helper)

docs/
└── architecture.md                  # ADD "Reconciler" subsection: status condition vocabulary + leader election shape
```

**Structure Decision**: 

A new top-level `crates/operator/src/leader.rs` module — rather than inlining leader-election in `main.rs` — keeps `main.rs` thin (it's strictly bootstrap wiring) and isolates leader-election from reconcile logic so the two can evolve independently. Centralizing condition writes in `status.rs` (currently a one-line bootstrap stub) means feature 003's `reason=Scanning`, feature 007's `reason=ImageDiffed`, etc. all extend the same enum and use the same patch helper — no scattered `serde_json!({...})` calls across the codebase.
