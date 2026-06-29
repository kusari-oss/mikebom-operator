# Phase 0 Research: Reconciler spawns scan Jobs

This document records the decisions feature 007 makes before code lands. Each
decision is short: what we're doing, why, and what we considered. The plan
references each by its number (`research.md §N`).

## 1. Job naming

- **Decision**: Reuse feature 003's `scan_job::job_name(cr_name, &short_hash)` exactly. The reconciler computes the same name via a `pub(crate)` re-export — *not* a copy.
- **Rationale**: FR-004 requires deterministic naming for the get-before-create check. Feature 003 already produces DNS-1123-sanitized names of the form `nsscan-<cr>-<7chr-hash>`, capped at 63 chars. Reusing prevents drift between builder and reconciler.
- **Alternatives**: (a) Compute names in the orchestrator separately — rejected; risks drift on rename. (b) Inspect `job.metadata.name` after `build_scan_job` to learn the name — works but obscures the source of truth.

## 2. OwnerReference construction

- **Decision**: Build `OwnerReference { apiVersion: "kusari.dev/v1alpha1", kind: "NamespaceScan", name: cr.name, uid: cr.uid, controller: Some(true), block_owner_deletion: Some(true) }` and inject into `job.metadata.owner_references` *after* `build_scan_job()` returns.
- **Rationale**: FR-005 requires Kubernetes-side GC on CR delete. Setting `controller=true` + `blockOwnerDeletion=true` is the standard pattern. Doing this outside the builder keeps `build_scan_job` a pure data transform per feature 003's contract.
- **Alternatives**: Building ownerRefs inside `build_scan_job` — rejected; the builder is intentionally I/O-free and CR-meta-free, and feature 008's unit tests benefit from that purity.

## 3. Idempotent create

- **Decision**: Call `Api::<Job>::create(&PostParams::default(), &job).await`. On `kube::Error::Api(ErrorResponse { code: 409, .. })`, treat as `Ok(false)` (job preexisting). On `kube::Error::Api(ErrorResponse { code: 403, .. })`, surface `RbacInsufficient`. Otherwise propagate.
- **Rationale**: 409 = name collision = "another reconcile or replica already created this Job"; that's the success state for the get-before-create invariant. Skipping the explicit `get` saves a round-trip and avoids TOCTOU.
- **Alternatives**: (a) Get-then-create — rejected; race window + extra API call. (b) Apply patch — rejected; Jobs are immutable on most spec fields, so Apply often 422s.

## 4. Pod phase filter

- **Decision**: A pod is in scope iff `pod.status.phase ∈ { Some("Running"), Some("Pending"), None }`. `None` is treated as `Pending`-equivalent for pods the API server hasn't fully populated yet.
- **Rationale**: Spec clarification (2026-06-28 session) — Running + Pending only. The `None` accommodation is a defensive choice for watch-list freshness; if upstream populates phase before the watch fires for our list, we'll never see `None` in practice.
- **Alternatives**: All phases (rejected — pollutes SBOM scope with finished work). Running only (rejected — adds 5min latency for newly-applied workloads).

## 5. Image dedup scope

- **Decision**: Collect strings from `pod.spec.init_containers[].image`, `pod.spec.containers[].image`, and `pod.spec.ephemeral_containers[].image`. Use a `BTreeSet<String>` (ordered for stable test assertions). Skip empty/None strings. Apply no normalization — `nginx:1.27.0` and `nginx@sha256:abc...` are distinct dedup keys even when they resolve to the same image.
- **Rationale**: Edge case in spec: "Pod uses an image with no tag and no digest" → MUST NOT silently inject `:latest`. Treating the literal string as the key honors that. Including ephemeral containers makes debug-attach images visible in the scan graph.
- **Alternatives**: Containers-only (rejected — misses init-time deps and ephemeral debug images). Reference-canonicalization (rejected — would mask user intent and break reproducibility).

## 6. RBAC fail-closed semantics

- **Decision**: A single 403 from any pod-list call or any Job-create call aborts the whole orchestration with `OrchestrationResult::RbacInsufficient { verb_resource, namespace, message }`. No partial spawn — if RBAC permits namespace A's pod list but not namespace B's, *zero* Jobs are spawned.
- **Rationale**: Constitution III is explicit: "MUST NEVER silently fall back to broader RBAC or broader namespace selection." Spawning Jobs for accessible namespaces while reporting RBACInsufficient for inaccessible ones would be "silent degradation" of the requested scope.
- **Alternatives**: Per-namespace partial spawn with multi-namespace error reporting — rejected; constitution III bars it.

## 7. `BuildFailed` vs `RBACInsufficient` split

- **Decision**: Builder errors (`BuildScanJobError::*`) become `OrchestrationResult::BuildFailed { image_ref, error }` → status reason `BuildFailed`. Kube 403s become `OrchestrationResult::RbacInsufficient { .. }` → status reason `RBACInsufficient`. Both are surfaced with distinct, non-collapsing reasons.
- **Rationale**: Different remediation paths. `BuildFailed` (e.g., `MissingPvcConfig`) means the user must fix the CR's `output` block. `RBACInsufficient` means the cluster admin must adjust RBAC. Conflating them would force the user to read the message to know what to do.
- **Alternatives**: Single `Error` reason — rejected; loses remediation signal.

## 8. Status-reason decision table

| Pre-call base reason (feature 002) | `OrchestrationResult` | Final reason |
|---|---|---|
| `InvalidSpec` | (not called) | `InvalidSpec` (preserved) |
| `NotYetReconciled` | `Spawned { n }` (n ≥ 1) | `Scanning` |
| `NotYetReconciled` | `NoImagesInScope` | `NoImagesInScope` |
| `NotYetReconciled` | `BuildFailed { .. }` | `BuildFailed` |
| `NotYetReconciled` | `RbacInsufficient { .. }` | `RBACInsufficient` |

- **Decision**: Encoded in a new `status::status_with_orchestration_result(base, result, now)` function. The base status is whatever `desired_status()` produced; the orchestration result post-processes it.
- **Rationale**: Keeps feature 002's `desired_status` untouched (FR-012). Single function = single decision table = easy to unit-test.
- **Alternatives**: Inline the mapping inside `reconcile()` — rejected; harder to test, mixes I/O with logic.

## 9. Operator namespace discovery

- **Decision**: `Ctx` gains an `operator_namespace: String` field, populated in `main.rs` from `POD_NAMESPACE` (the chart's deployment already injects this via downward API per `charts/mikebom-operator/templates/deployment.yaml`). Jobs are created via `Api::<Job>::namespaced(client, &ctx.operator_namespace)`.
- **Rationale**: FR-003 requires Jobs land in the operator's own namespace. `POD_NAMESPACE` is the standard pattern. The chart already sets it; no chart edit needed.
- **Alternatives**: `Api::<Job>::default_namespaced(client)` — rejected; binds to the kubeconfig's default namespace which isn't necessarily the operator's. Reading the Lease's namespace — works but couples Job placement to leader election.

## 10. In-process integration test approach

- **Decision**: New `e2e/tests/reconciler_spawns_job.rs` (gated by `MIKEBOM_OPERATOR_E2E=1`) uses `kube::Client::try_default()` against a kind cluster, applies fixtures via `Api`, and invokes `scan_orchestrator::ensure_jobs(...)` directly in-process. No operator pod, no chart install, no Docker image build.
- **Rationale**: Constitution VI requires kind-based E2E for reconciler-logic changes. The chart-install path (constitution dev-workflow's `namespace_scan_baseline`) is heavy and overlaps with future features. The in-process variant exercises the same kube API code path as production but runs in <10s per case.
- **Alternatives**: Full helm-install + operator-image-load — deferred to a later feature when more end-to-end behavior exists to justify the harness. Pure unit tests with a fake `kube::Client` — covers the logic but not constitution VI's "kind-based E2E" mandate.
