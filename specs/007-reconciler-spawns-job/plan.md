# Implementation Plan: Reconciler spawns scan Jobs

**Branch**: `007-reconciler-spawns-job` | **Date**: 2026-06-28 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/007-reconciler-spawns-job/spec.md`

## Summary

Wire feature 002's reconciler into feature 003–006's `build_scan_job` builder. The reconciler, on each cycle for a valid CR, lists pods in the target namespaces, filters by `status.phase ∈ {Running, Pending}` (per the 2026-06-28 clarification), collects the deduplicated set of container images across `initContainers`/`containers`/`ephemeralContainers`, and for each image performs an idempotent get-or-create of a `batch/v1.Job` in the operator's own namespace. Each spawned Job carries an `ownerReference` to the CR (`controller=true`, `blockOwnerDeletion=true`) so Kubernetes garbage-collects them on CR deletion. Status reasons gain three new values — `Scanning`, `NoImagesInScope`, `RBACInsufficient` — joining feature 002's `NotYetReconciled` and `InvalidSpec`. No CRD shape changes; no Helm chart RBAC changes (the existing ClusterRole already grants `pods:list,watch` and `jobs:create,get,list,delete`). The next feature (008) layers Job status feedback (`ScanCompleted` / `ScanFailed`); 007 explicitly does not read Job status (FR-014) or honor `spec.schedule` (FR-015).

## Technical Context

**Language/Version**: Rust 1.85+ stable (workspace toolchain, same as features 001–006).

**Primary Dependencies**:
- `kube` 0.97 — `Api::<Pod>::namespaced(...).list(&ListParams)`, `Api::<Job>::namespaced(...).create(&PostParams, &job)`. Already in workspace.
- `k8s-openapi` 0.23 with `v1_31` feature — `core::v1::Pod`, `batch::v1::Job`, `meta::v1::OwnerReference`. Already in workspace.
- No new direct deps. Feature 007 reuses the kube-rs primitives feature 002 already depends on.

**Storage**: N/A — reconciler is pure orchestration. Jobs and their pods are the only new in-cluster state, and they're produced by feature 003's pure builder.

**Testing**:
- **Unit tests** (in `crates/operator/src/reconcile/scan_orchestrator.rs`): pure-function tests for image enumeration (HashSet dedup), phase filtering, owner-reference construction, and the new status-reason decision table. No kube client needed.
- **Integration test** (new `e2e/tests/reconciler_spawns_job.rs`, gated by `MIKEBOM_OPERATOR_E2E=1`): uses `kube::Client::try_default()` against a kind cluster, applies a fixture `NamespaceScan` CR + fixture pods directly via `kube::Api`, invokes the orchestrator function in-process (no operator-image build), and asserts (a) the expected Jobs exist, (b) their `ownerReferences` point at the CR, (c) a second invocation creates zero new Jobs. Avoids the heavy chart-install path; covers constitution VI for the reconciler-logic touch.
- The existing `e2e/tests/scan_job_dryrun.rs` is not modified (its builder-only assertions are still valid as-is).

**Target Platform**: Linux x86_64 / macOS dev — same as features 001–006.

**Project Type**: Rust workspace — implementation lives in the existing `operator` crate's `reconcile` submodule.

**Performance Goals**:
- Builder + reconcile loop completes in <30 seconds for 25 distinct images across 100 pods (SC-001 / FR-011).
- Single-pod list call per target namespace; image enumeration is O(pods × containers-per-pod).
- Job creates are sequential (25 sequential API calls × ~50ms each ≈ 1.3s; well inside the 30s budget — no concurrent-creates complexity needed for v0.7).

**Constraints**:
- All feature 002 tests MUST continue to pass — `desired_status()` keeps its existing signature; the new orchestrator augments rather than replaces it. Feature 002's `lib`/E2E test count stays green.
- All feature 003–006 `scan_job` tests MUST continue to pass — `build_scan_job` is called as-is; we add the `ownerReferences` field to the returned `Job` *after* the builder runs, never inside it.
- `BuildScanJobError::*` variants the reconciler now exposes to the user: the orchestrator MUST distinguish "builder rejected the spec" (e.g., `MissingPvcConfig`) from "kube API rejected the create" (e.g., 403). The status-reason mapping is non-collapsing: builder errors surface as a new `BuildFailed` reason; API 403s surface as `RBACInsufficient`.
- Constitution I/II: no new C deps, no mikebom-internal coupling, no SBOM parsing.

**Scale/Scope**:
- v0.7 target: one CR, ≤100 pods spanning ≤25 distinct images per CR (FR-011, SC-001). Multi-CR concurrency is bounded by kube-rs Controller's default `concurrency` (= 1 for `NamespaceScan` reconciliation per feature 002's setup); no explicit semaphore.
- The orchestrator's working set: `HashSet<String>` of image refs (<2 KB even for 25 images) + a `Vec<Job>` snapshot of existing same-CR Jobs returned by label-selector list. Memory footprint is negligible.

## Constitution Check

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Pure Rust where reasonable | PASS | No new deps; existing pure-Rust stack. |
| II. USE not EMBED (NON-NEGOTIABLE) | PASS | Reconciler doesn't touch mikebom internals; it shells out via the existing `mikebom-scan` container in the spawned Job. |
| III. Fail Closed on RBAC (NON-NEGOTIABLE) | PASS | FR-008 is explicit: any 403 from `pods:list` or `jobs:create` surfaces `RBACInsufficient` with no Jobs spawned. The orchestrator MUST treat a partial RBAC grant (e.g., access to namespace A but not B) as fail-closed across the entire CR — no Jobs for any image. |
| IV. CRD Backward Compatibility | PASS | No CRD shape changes. Three new condition reasons (`Scanning`, `NoImagesInScope`, `RBACInsufficient`) are all reserved already in `docs/architecture.md` and not surfaced by feature 002, so transitions are additive. |
| V. SBOM-Format Agnostic | PASS | Reconciler doesn't parse SBOMs; only orchestrates Job creation. |
| VI. Hermetic E2E Tests (NON-NEGOTIABLE) | PASS | New gated E2E exercises the reconciler against a kind cluster end-to-end: apply CR → spawn Jobs → assert owner-refs + idempotency. The chart-installed full-cluster baseline (constitution dev-workflow `namespace_scan_baseline`) is separate; this feature's E2E is the in-process integration variant. |
| VII. Helm Chart Lockstep | PASS | No CRD shape changes (chart YAML drift check stays green). Existing chart RBAC already grants `pods:get,list,watch` cluster-wide and `jobs:create,get,list,watch,delete` cluster-wide — confirmed by grepping `charts/mikebom-operator/templates/rbac.yaml`. The spec's Assumptions section claimed a chart RBAC update was required; this plan corrects that — no chart changes are needed. |

All gates pass. No `## Complexity Tracking` section needed.

## Project Structure

### Documentation (this feature)

```text
specs/007-reconciler-spawns-job/
├── plan.md                              # this file
├── spec.md                              # spec (with 2026-06-28 phase-filter clarification)
├── research.md                          # Phase 0: 10 decisions (Job naming reuse, ownerRefs construction, idempotent create, pod phase filter, image dedup scope, RBAC fail-closed semantics, BuildFailed vs RBACInsufficient split, status-reason decision table, operator-namespace discovery, in-process integration test approach)
├── data-model.md                        # Phase 1: types added (Ctx extension, status-reason set, orchestration result), FR→test mapping
├── quickstart.md                        # Phase 1: admin install/upgrade flow (chart-install no-op since RBAC unchanged) + contributor "how to add a new orchestration step" notes
├── contracts/
│   └── reconciler-orchestrator.md       # Internal contract: `ensure_jobs(spec, cr_meta, ctx) -> OrchestrationResult`
└── tasks.md                             # /speckit-tasks output (not created here)

```

### Source Code (repository root)

```text
crates/operator/src/
├── main.rs                              # MODIFY (small):
│                                        #   - Extend `Ctx { client, operator_namespace }`
│                                        #   - Pass `pod_namespace` into Ctx alongside the LeaderConfig field
│
├── reconcile/
│   ├── mod.rs                           # MODIFY: re-export new `scan_orchestrator` submodule
│   ├── namespace_scan.rs                # MODIFY:
│   │                                    #   - On valid spec: call `scan_orchestrator::ensure_jobs(...)`
│   │                                    #   - On InvalidSpec (per feature 002): skip orchestration entirely
│   │                                    #   - Translate OrchestrationResult into a status patch (reason mapping below)
│   │                                    #   - Ctx struct gains `operator_namespace: String`
│   ├── scan_orchestrator.rs             # NEW:
│   │                                    #   - `pub async fn ensure_jobs(spec, cr_meta, ctx) -> Result<OrchestrationResult, OrchestrationError>`
│   │                                    #   - `pub enum OrchestrationResult { Spawned { distinct_images: usize }, NoImagesInScope, BuildFailed(BuildScanJobError), RbacInsufficient(String) }`
│   │                                    #   - Pure helpers (unit-tested):
│   │                                    #       * `collect_images_from_pods(pods: &[Pod]) -> BTreeSet<String>` (init + containers + ephemeral, phase-filtered)
│   │                                    #       * `is_pod_in_scope(pod: &Pod) -> bool` (phase ∈ {Running, Pending})
│   │                                    #       * `make_owner_reference(cr: &NamespaceScan) -> OwnerReference`
│   │                                    #   - I/O helpers (integration-tested):
│   │                                    #       * `list_target_pods(api: &Api<Pod>, target: &Target) -> Result<Vec<Pod>, OrchestrationError>`
│   │                                    #       * `try_create_job_idempotent(api: &Api<Job>, job: Job) -> Result<bool, OrchestrationError>` (409 → false = preexisting)
│   ├── image_diff.rs                    # UNCHANGED (placeholder from feature 002 ages — left as-is)
│   └── job_template.rs                  # UNCHANGED (placeholder from feature 002 — left as-is)
│
├── status.rs                            # MODIFY (small):
│                                        #   - Add new condition reasons as `pub const` strings:
│                                        #       SCANNING / NO_IMAGES_IN_SCOPE / RBAC_INSUFFICIENT / BUILD_FAILED
│                                        #   - Existing `desired_status()` keeps its signature; orchestrator-driven reasons are written via a new
│                                        #     `pub fn status_with_orchestration_result(base: NamespaceScanStatus, result: &OrchestrationResult, now: DateTime<Utc>) -> NamespaceScanStatus`
│
└── scan_job/mod.rs                      # UNCHANGED. The reconciler is a new caller; the builder API is stable per feature 003's contract.

e2e/tests/
├── reconciler_spawns_job.rs             # NEW (gated): in-process integration test against kind
│                                        #   - Applies a fixture CR + 3 fixture pods (2 distinct images, 1 phase=Succeeded that should be excluded)
│                                        #   - Calls `ensure_jobs(...)` directly
│                                        #   - Asserts: 2 Jobs exist, both owned by the CR, second invocation creates 0 new Jobs
│                                        #   - Tests `OrchestrationResult::NoImagesInScope` against a namespace with only a Succeeded pod
└── scan_job_dryrun.rs                   # UNCHANGED (its builder-only assertions remain valid)

charts/mikebom-operator/
└── (no changes)                         # Existing rbac.yaml already grants pods + jobs. CRD YAML unchanged → drift check stays green.

docs/
├── architecture.md                      # MODIFY: update the "Reasons reserved for future features" table — move Scanning + RBACInsufficient + NoImagesInScope into "written by feature 007"
└── crd-reference.md                     # MODIFY: add NoImagesInScope to the table of condition reasons; reference feature 007 for it and Scanning
```

**Structure Decision**:

A new `scan_orchestrator.rs` sibling under `reconcile/` houses the new logic. Reasoning:

- `reconcile/namespace_scan.rs` stays focused on "watch CR, patch status, requeue" — adding ~150 lines of image enumeration + Job creation would push it past the readability threshold I called out in earlier features' plans.
- The orchestrator is independently testable: its pure helpers (image collection, phase filter, owner-ref construction) live behind a clean function boundary, so unit tests don't need a kube fixture.
- The `OrchestrationResult` enum lives with the orchestrator and is consumed only by `namespace_scan.rs::reconcile`. It is not in the public API surface of the `operator` crate.

Following feature 003's plan precedent: if `scan_orchestrator.rs` grows past ~500 lines as features 008+ layer on Job-status watching, we'll split the pure helpers into `image_set.rs` and keep the I/O in `scan_orchestrator.rs`. For v0.7 a single file is right-sized.

## Phase 0: Outline & Research

Research artifact: [research.md](./research.md). The 10 decisions it records:

1. **Job naming**: reuse feature 003's `job_name(cr_name, &short_hash)` exactly. The reconciler computes the same name (via a re-exported helper) for the get-before-create check. Decision: re-export `scan_job::job_name` as `pub(crate)`. Alternatives considered: separate name generation in the orchestrator (rejected — risks drift).

2. **OwnerReference construction**: built post-`build_scan_job()` and injected into the returned `Job`'s `metadata.owner_references`. Field values: `apiVersion = "kusari.dev/v1alpha1"`, `kind = "NamespaceScan"`, `name = cr.name`, `uid = cr.uid`, `controller = true`, `blockOwnerDeletion = true`. Alternatives considered: setting them inside `build_scan_job` (rejected — the builder stays a pure data transform; ownership is a runtime concern).

3. **Idempotent create**: try `Api::<Job>::create(&PostParams::default(), &job).await`; on `kube::Error::Api(ErrorResponse { code: 409, .. })` treat as success (Job already exists from a prior reconcile or a racing replica). No explicit `get` first — saves one API call and is the kube-rs idiom. Alternatives considered: get-then-create (rejected — TOCTOU + extra round-trip), `apply` patch (rejected — Jobs are immutable on most fields, would conflict).

4. **Pod phase filter**: `pod.status.phase` ∈ {`Some("Running")`, `Some("Pending")`}. Pods with `None` phase (not yet visible to apiserver) are treated as `Pending`-equivalent in scope. Per the 2026-06-28 spec clarification.

5. **Image dedup scope**: collect from `pod.spec.init_containers[].image`, `pod.spec.containers[].image`, `pod.spec.ephemeral_containers[].image`. Use a `BTreeSet<String>` (ordered for stable test assertions). Empty/None image strings are skipped. No trimming or normalization — the literal string is the dedup key per Edge Cases.

6. **RBAC fail-closed semantics**: a single 403 from *any* pod-list call or *any* Job-create call aborts the whole orchestration with `RBACInsufficient`. No partial spawn. The message includes the kube `ErrorResponse.message` verbatim, plus the verb + resource + namespace. Per constitution III + FR-008.

7. **`BuildFailed` vs `RBACInsufficient` split**: feature 003–006's `BuildScanJobError` (e.g., `MissingPvcConfig`) is a *spec* problem the user must fix; it surfaces as a new `BuildFailed` reason, distinct from `RBACInsufficient` which is a *cluster* problem. The reason message names the failing image ref and the builder error variant. Alternative considered: collapsing both into `Error` (rejected — different user remediation).

8. **Status-reason decision table** (drives `status_with_orchestration_result`):

   | Pre-call base reason (from feature 002) | OrchestrationResult | Final reason |
   |---|---|---|
   | `InvalidSpec` | (not called) | `InvalidSpec` (preserved) |
   | `NotYetReconciled` | `Spawned { n }` (n ≥ 1) | `Scanning` |
   | `NotYetReconciled` | `NoImagesInScope` | `NoImagesInScope` |
   | `NotYetReconciled` | `BuildFailed(_)` | `BuildFailed` |
   | `NotYetReconciled` | `RbacInsufficient(_)` | `RBACInsufficient` |

   The pre-call base reason is always `NotYetReconciled` for valid specs (feature 002's invariant). Future features can override (e.g., feature 008's `ScanCompleted` would post-process the `Scanning` outcome).

9. **Operator namespace discovery**: read from `Ctx::operator_namespace`, which `main.rs` populates from `POD_NAMESPACE` (already injected by the chart's deployment via downward API — confirmed in `charts/mikebom-operator/templates/deployment.yaml`). Fallback to `"default"` only outside a cluster (dev mode). Jobs are created via `Api::<Job>::namespaced(client, &ctx.operator_namespace)`.

10. **In-process integration test approach**: the gated `e2e/tests/reconciler_spawns_job.rs` uses `kube::Client::try_default()` against a kind cluster, applies fixtures via `Api`, and invokes `scan_orchestrator::ensure_jobs(...)` directly. No chart install, no operator pod, no image build. Alternative considered: full helm-install path (deferred — that's the broader `namespace_scan_baseline` test in the constitution's dev-workflow, which will be authored in a later feature when more behavior is end-to-end). The in-process variant gives us tight integration coverage with sub-10s test runtime per case.

**Output**: research.md with all 10 decisions resolved. No `NEEDS CLARIFICATION` markers remain.

## Phase 1: Design & Contracts

**Prerequisites**: research.md complete.

### Data model

[data-model.md](./data-model.md) captures:

- **`Ctx` (modified)**: `pub struct Ctx { client: Client, operator_namespace: String }`. The added field is used only by `scan_orchestrator::ensure_jobs(...)`. Existing call sites compile after passing `operator_namespace` from `main.rs`.

- **`OrchestrationResult` (new)**:
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum OrchestrationResult {
      Spawned { distinct_images: usize },
      NoImagesInScope,
      BuildFailed { image_ref: String, error: String }, // BuildScanJobError stringified
      RbacInsufficient { verb_resource: String, namespace: Option<String>, message: String },
  }
  ```
  Not `#[non_exhaustive]` — kept internal to the `operator` crate.

- **`OrchestrationError` (new)**: a `thiserror`-derived enum wrapping `kube::Error` and `BuildScanJobError`. The orchestrator returns `Err` only for unexpected/transient kube errors (e.g., 500s); it returns `Ok(OrchestrationResult::RbacInsufficient {..})` for 403 and `Ok(OrchestrationResult::BuildFailed {..})` for builder errors so the reconciler's status patch path is the single source of reason mapping.

- **Status-reason constants (new in `status.rs`)**: `pub const SCANNING: &str = "Scanning"; …` for the four new reasons. Existing constants (`INVALID_SPEC`, `NOT_YET_RECONCILED`) keep their names.

- **FR → test mapping**:
  - FR-001/edge cases → `collect_images_from_pods` unit tests (init + containers + ephemeral + phase filter + empty/None handling)
  - FR-002, FR-004 → `try_create_job_idempotent` integration test + Job-name re-export round-trip unit test
  - FR-003 → integration test asserts `job.metadata.namespace == Ctx::operator_namespace`
  - FR-005 → integration test asserts `job.metadata.owner_references[0]` shape
  - FR-006, FR-007 → reason-mapping unit tests over `status_with_orchestration_result` (no kube needed)
  - FR-008 → integration test using a `pods:list`-denied ServiceAccount in kind (alternative for kind portability: inject a fake `Client` that returns 403 in a unit test — research.md picks the unit-test path for v0.7)
  - FR-009 → integration test runs `ensure_jobs` twice; second invocation MUST report `Spawned { distinct_images: 0 }` (or a dedicated `NoOp` variant — decided in data-model.md)
  - FR-010 → unit test feeds a fake `Api<Job>` returning 409 on `create`; `try_create_job_idempotent` returns `Ok(false)`
  - FR-011 → integration test asserts wall-clock <30s for 25 fixture images (kind-side budget; not exact but bounds order-of-magnitude regressions)
  - FR-012 → existing feature 002 lib tests + the unchanged `e2e/tests/reconciler_skeleton.rs` continue to pass
  - FR-013 → existing feature 003–006 `scan_job::tests::*` continue to pass; new orchestrator tests do NOT modify the builder
  - FR-014 → no test asserts Job status; future feature 008's tests will
  - FR-015 → no test consults `spec.schedule`; the orchestrator never reads it

### Contracts

[contracts/reconciler-orchestrator.md](./contracts/reconciler-orchestrator.md) — internal contract for `ensure_jobs`. Not a public crate API, but worth fixing so features 008+ can layer cleanly. Pins:

- Function signature: `pub async fn ensure_jobs(spec: &NamespaceScanSpec, cr_meta: &CrMetaSnapshot, ctx: &Ctx) -> Result<OrchestrationResult, OrchestrationError>`.
- `CrMetaSnapshot { name: String, uid: String, namespace: String }` — minimal subset of `ObjectMeta` the orchestrator needs (lets unit tests synthesize it without a full `NamespaceScan`).
- Idempotency invariant: for fixed inputs, two sequential calls produce the same `OrchestrationResult` variant with `distinct_images` reflecting only the *first* call's spawns (the second sees them as preexisting via the 409 branch).
- Side-effect bound: the function reads pods in `spec.target.namespaces`, creates Jobs in `ctx.operator_namespace`, and writes nothing else. It MUST NOT patch status (that's `reconcile()`'s job).

### Agent context update

The project's `CLAUDE.md` currently has `Active plan: [specs/004-pvc-backend/plan.md](specs/004-pvc-backend/plan.md)`. Phase 1 updates this to point at the new plan via the `<!-- SPECKIT START --><!-- SPECKIT END -->` markers (or, if the markers don't exist in CLAUDE.md, the same single line gets edited). Done via the speckit agent-context extension after this plan lands.

**Output**: data-model.md, contracts/reconciler-orchestrator.md, quickstart.md, updated `CLAUDE.md`.

## Re-evaluate Constitution Check (post-design)

Re-checking after Phase 1:

| Principle | Status | Notes |
|-----------|--------|-------|
| I | PASS | Confirmed: zero new direct deps. |
| II | PASS | Confirmed: `ensure_jobs` never touches mikebom internals. |
| III | PASS | The `OrchestrationResult::RbacInsufficient` arm + the status-reason decision table together guarantee no Jobs are spawned when any RBAC check fails. The contract's "side-effect bound" clause forbids partial spawn. |
| IV | PASS | No CRD shape changes. The four new reasons are reserved per `docs/architecture.md` and additive. |
| V | PASS | No SBOM access. |
| VI | PASS | Gated in-process integration test covers the reconciler-logic touch. |
| VII | PASS | No CRD changes; no chart changes. Drift check unaffected. |

All gates still pass post-design. No complexity tracking needed.
