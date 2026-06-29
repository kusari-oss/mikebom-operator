---

description: "Task list for feature 007: reconciler spawns scan Jobs"
---

# Tasks: Reconciler spawns scan Jobs

**Input**: Design documents from `/specs/007-reconciler-spawns-job/`

**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/reconciler-orchestrator.md, quickstart.md

**Tests**: Included. The mikebom-operator repo follows the same TDD-style discipline as features 001–006 — unit tests inline with implementation modules, integration tests under `e2e/` gated by `MIKEBOM_OPERATOR_E2E=1`. Tests are written first within each phase.

**Organization**: Tasks are grouped by user story. US1 is the MVP; US2 and US3 are P2 increments layered on top.

## Format: `[ID] [P?] [Story?] Description with file path`

- **[P]**: Can run in parallel (different files, no in-flight dependency).
- **[Story]**: Required on Phase 3+ tasks (US1 / US2 / US3).

## Path Conventions

Rust workspace with the `operator` crate at `crates/operator/`. The `e2e` crate at `e2e/`. All paths are repo-relative.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Carve out the new module home and extend the reconciler context so later phases have somewhere to land.

- [X] T001 Extend `Ctx` struct with `operator_namespace: String` field in `crates/operator/src/reconcile/namespace_scan.rs`
- [X] T002 Populate the new `operator_namespace` field from `POD_NAMESPACE` env var when constructing `Ctx` in `crates/operator/src/main.rs`
- [X] T003 Create empty `scan_orchestrator` module skeleton at `crates/operator/src/reconcile/scan_orchestrator.rs` and register it in `crates/operator/src/reconcile/mod.rs`

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Lay down the type system and constants every user story depends on — status-reason strings, the orchestrator result/error enums, and the small re-exports the orchestrator needs from `scan_job`.

**⚠️ CRITICAL**: No user story work can begin until this phase is complete.

- [X] T004 [P] Add status reason constants (`SCANNING`, `NO_IMAGES_IN_SCOPE`, `RBAC_INSUFFICIENT`, `BUILD_FAILED`) to `crates/operator/src/status.rs` alongside existing `NOT_YET_RECONCILED` / `INVALID_SPEC`
- [X] T005 [P] Define `OrchestrationResult` enum (`Spawned { distinct_images }`, `NoImagesInScope`, `BuildFailed { image_ref, error }`, `RbacInsufficient { verb_resource, namespace, message }`) per data-model.md in `crates/operator/src/reconcile/scan_orchestrator.rs`
- [X] T006 [P] Define `OrchestrationError` enum (`#[from] kube::Error` only — 403/409 absorbed inline) in `crates/operator/src/reconcile/scan_orchestrator.rs`
- [X] T007 [P] Define `CrMetaSnapshot { name, uid, namespace }` struct in `crates/operator/src/reconcile/scan_orchestrator.rs`
- [X] T008 Re-export `scan_job::job_name` and `scan_job::short_image_hash` as `pub(crate)` in `crates/operator/src/scan_job/mod.rs` so the orchestrator can compute identical Job names without duplicating logic (research.md §1)

**Checkpoint**: Module skeleton + types exist; user stories can land in parallel.

---

## Phase 3: User Story 1 — Operator spawns a scan Job per target image (Priority: P1) 🎯 MVP

**Story goal**: For every valid `NamespaceScan` CR, enumerate in-scope pods (phase `Running`/`Pending`), dedupe their images, and create one `batch/v1.Job` per image in the operator's namespace with `ownerReferences` pointing at the CR.

**Independent Test**: With a kind cluster running, apply a CR + 3 fixture pods (2 distinct images, 1 in phase `Succeeded` that MUST be excluded). Call `ensure_jobs(...)` once. Expect exactly 2 Jobs in the operator's namespace, each owned by the CR via `controller=true` ownerRef. (Maps to SC-001, SC-003, FR-001 through FR-005.)

**Tests first (unit):**

- [X] T009 [P] [US1] Unit tests for `is_pod_in_scope` (phase filter — Running ✓, Pending ✓, Succeeded ✗, Failed ✗, Unknown ✗, None → Pending-equivalent ✓) in `crates/operator/src/reconcile/scan_orchestrator.rs::tests`
- [X] T010 [P] [US1] Unit tests for `collect_images_from_pods` (dedup across pods, init+main+ephemeral all contribute, empty/None image strings skipped, phase filter applied) in `crates/operator/src/reconcile/scan_orchestrator.rs::tests`
- [X] T011 [P] [US1] Unit tests for `make_owner_reference` (apiVersion=`kusari.dev/v1alpha1`, kind=`NamespaceScan`, controller=true, blockOwnerDeletion=true, name/uid from `CrMetaSnapshot`) in `crates/operator/src/reconcile/scan_orchestrator.rs::tests`
- [X] T012 [P] [US1] Unit test for `try_create_job_idempotent` against a fake `Api<Job>` returning 409 → `Ok(false)` (preexisting) in `crates/operator/src/reconcile/scan_orchestrator.rs::tests`
- [X] T012b [P] [US1] Unit tests for the **403 fail-closed path** (closes the H1 audit gap on constitution principle III): `list_target_pods` against a fake `Api<Pod>` returning 403 → `Err(RbacInsufficient { verb_resource: "list pods", namespace: Some(target_ns), .. })`; `try_create_job_idempotent` against a fake `Api<Job>` returning 403 → `Err(RbacInsufficient { verb_resource: "create batch/v1.jobs", .. })`. Both messages MUST verbatim include the kube `ErrorResponse.message` per contract invariant. File: `crates/operator/src/reconcile/scan_orchestrator.rs::tests` (FR-008, SC-005)
- [X] T013 [P] [US1] Unit test that orchestrator-computed Job name matches `scan_job::job_name(&cr_name, &scan_job::short_image_hash(&image_ref))` for matched inputs in `crates/operator/src/reconcile/scan_orchestrator.rs::tests` (FR-004)

**Implementation:**

- [X] T014 [P] [US1] Implement pure helper `fn is_pod_in_scope(pod: &Pod) -> bool` in `crates/operator/src/reconcile/scan_orchestrator.rs`
- [X] T015 [P] [US1] Implement pure helper `fn collect_images_from_pods(pods: &[Pod]) -> BTreeSet<String>` in `crates/operator/src/reconcile/scan_orchestrator.rs`
- [X] T016 [P] [US1] Implement pure helper `fn make_owner_reference(cr_meta: &CrMetaSnapshot) -> OwnerReference` in `crates/operator/src/reconcile/scan_orchestrator.rs`
- [X] T017 [US1] Implement async I/O helper `async fn list_target_pods(client: &Client, target: &Target) -> Result<Vec<Pod>, OrchestrationResult>` (returns `Err(RbacInsufficient {..})` on 403; `Err(NoImagesInScope)` on 404; bubble other errors) in `crates/operator/src/reconcile/scan_orchestrator.rs`
- [X] T018 [US1] Implement async I/O helper `async fn try_create_job_idempotent(api: &Api<Job>, job: Job) -> Result<bool, OrchestrationResult>` (Ok(true)=created, Ok(false)=preexisting via 409, Err=RbacInsufficient on 403) in `crates/operator/src/reconcile/scan_orchestrator.rs`
- [X] T019 [US1] Implement glue `pub async fn ensure_jobs(spec: &NamespaceScanSpec, cr_meta: &CrMetaSnapshot, ctx: &Ctx) -> Result<OrchestrationResult, OrchestrationError>` per contract — list pods first, dedupe, build+ownerRef-stamp each Job, create idempotently, short-circuit on first RBAC/Build failure (invariant #1 in contract) in `crates/operator/src/reconcile/scan_orchestrator.rs`
- [X] T020 [US1] Call `scan_orchestrator::ensure_jobs(...)` from `reconcile()` after `desired_status()` returns a non-`InvalidSpec` base, in `crates/operator/src/reconcile/namespace_scan.rs` (the orchestrator result is parked for US2 to consume; for US1 just calling it is enough to satisfy "Jobs get spawned")

**Tests last (E2E, gated):**

- [X] T021 [US1] Add kind-based integration test `spawns_one_job_per_distinct_in_scope_image` (gated by `MIKEBOM_OPERATOR_E2E=1`) — applies CR + 3 fixture pods (2 distinct in-scope images, 1 phase=Succeeded), invokes `ensure_jobs` in-process, asserts 2 Jobs exist in the operator's namespace with the expected ownerRef shape — in new file `e2e/tests/reconciler_spawns_job.rs`
- [X] T021b [US1] Add kind-based integration test `spawns_at_scale_within_30s` (gated, closes the M1 scale-coverage gap on FR-011/SC-001) — applies CR + 25 distinct in-scope images (use programmatically generated pod fixtures with images of the form `registry.local/test/image-{i}:v1` for `i ∈ 1..=25`), invokes `ensure_jobs` once, asserts wall-clock duration <30 seconds AND `OrchestrationResult::Spawned { distinct_images: 25 }`. The asserted budget is loose (catches order-of-magnitude regressions, not microbenchmarks) — in `e2e/tests/reconciler_spawns_job.rs`

**Checkpoint**: After Phase 3, `kubectl get jobs -n kusari-operator -l kusari.dev/namespace-scan=<cr-name>` returns the expected Jobs. Status still says `NotYetReconciled` (US2 hasn't landed yet); CR-delete already cascades to Jobs via Kubernetes GC (US1 satisfies SC-003).

---

## Phase 4: User Story 2 — Status condition reflects scan in progress (Priority: P2)

**Story goal**: After the orchestrator runs, the CR's status condition transitions from `NotYetReconciled` to `Scanning` / `NoImagesInScope` / `BuildFailed` / `RBACInsufficient` based on the `OrchestrationResult`.

**Independent Test**: With Phase 3 landed, apply a CR that produces ≥1 Job. Within 10s, `kubectl get namespacescan <name> -o json | jq '.status.conditions[] | select(.type=="Ready").reason'` returns `Scanning`. With a CR targeting an empty namespace, the reason is `NoImagesInScope`. (Maps to SC-001 status assertion, SC-004.)

**Tests first (unit):**

- [X] T022 [P] [US2] Unit tests for `status_with_orchestration_result` mapping table — verify `Spawned { n: 1 }`→`Scanning`, `NoImagesInScope`→`NoImagesInScope`, `BuildFailed`→`BuildFailed` (message includes image_ref + error), `RbacInsufficient`→`RBACInsufficient` (message includes verb_resource + namespace), `InvalidSpec` base preserved (orchestrator not consulted) in `crates/operator/src/status.rs::tests`

**Implementation:**

- [X] T023 [US2] Implement `pub fn status_with_orchestration_result(base: NamespaceScanStatus, result: &OrchestrationResult, now: DateTime<Utc>) -> NamespaceScanStatus` in `crates/operator/src/status.rs` per the decision table in research.md §8
- [X] T024 [US2] In `reconcile()`, when the base reason is not `InvalidSpec`, apply `status_with_orchestration_result(...)` to the patched status (rather than the bare `desired_status` output) in `crates/operator/src/reconcile/namespace_scan.rs`

**Docs:**

- [X] T025 [P] [US2] Update `docs/architecture.md` — move `Scanning`, `RBACInsufficient`, `NoImagesInScope`, `BuildFailed` from the "reserved for future features" table into the "values written by the operator" table; cite feature 007
- [X] T026 [P] [US2] Add `Scanning`, `NoImagesInScope`, `BuildFailed`, `RBACInsufficient` rows to the "Condition reasons" table in `docs/crd-reference.md` with brief admin-facing meaning

**Tests last (E2E, gated):**

- [X] T027 [US2] Add kind-based integration test `status_reason_transitions_to_scanning_after_spawn` — applies CR, calls reconcile (in-process), expects `status.conditions[Ready].reason == "Scanning"` within bounded poll — in `e2e/tests/reconciler_spawns_job.rs`
- [X] T028 [US2] Add kind-based integration test `status_reason_no_images_in_scope_for_empty_namespace` — applies CR targeting a namespace with no in-scope pods, expects `reason == "NoImagesInScope"` — in `e2e/tests/reconciler_spawns_job.rs`

**Checkpoint**: After Phase 4, admins see meaningful status; the "lying status surface" critique from US2's "Why this priority" is closed.

---

## Phase 5: User Story 3 — Idempotent Job creation across reconciles (Priority: P2)

**Story goal**: Re-running reconcile against an unchanged CR + cluster state MUST NOT create duplicate Jobs.

**Independent Test**: Invoke `ensure_jobs(...)` twice against the same fixture. After the second call, the live Job count for that `(CR, image)` set is unchanged. (Maps to SC-002, FR-009, FR-010.)

The bulk of this story's logic landed in Phase 3 (T018's 409 handling + T019's get-or-create-by-deterministic-name flow). This phase adds the explicit E2E that proves the invariant end-to-end, plus one more unit test for the "Job exists from prior reconcile and has Succeeded" path (which the operator MUST NOT re-spawn — Acceptance Scenario 3.2 in spec.md).

**Tests first (unit):**

- [X] T029 [P] [US3] Unit test: `try_create_job_idempotent` with a fake `Api<Job>` returning success (no error) for a fresh Job → `Ok(true)`, then again for the same Job returning 409 → `Ok(false)` (idempotency over sequential calls) in `crates/operator/src/reconcile/scan_orchestrator.rs::tests`

**Tests last (E2E, gated):**

- [X] T030 [US3] Add kind-based integration test `ensure_jobs_is_idempotent_across_invocations` — applies CR + fixture pods, calls `ensure_jobs` twice in succession, asserts `OrchestrationResult` matches between calls AND total Job count after second call equals the first call's spawn count (no duplicates) — in `e2e/tests/reconciler_spawns_job.rs`
- [X] T031 [US3] Add kind-based integration test `ensure_jobs_does_not_respawn_for_completed_job` — applies CR + fixture pods, calls `ensure_jobs` to spawn a Job, marks the Job as `status.succeeded = 1` via `kubectl patch`, calls `ensure_jobs` again, asserts no new Job is created (spec Acceptance Scenario 3.2) — in `e2e/tests/reconciler_spawns_job.rs`

**Checkpoint**: After Phase 5, the operator survives long-running deployments without piling up Jobs. The 5-minute requeue cadence becomes safe.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Pre-PR gate, anti-regression, and the static checks the constitution requires.

- [X] T032 [P] Grep verification that no `Job.status` field is read anywhere in `crates/operator/src/reconcile/` (FR-014 — feature 008's scope) — fails the task if any of `.status.succeeded`, `.status.failed`, `.status.active` appears in production code
- [X] T033 [P] Grep verification that `spec.schedule` is not read in `crates/operator/src/reconcile/scan_orchestrator.rs` (FR-015) — fails if `schedule.cron` or `schedule.interval` appears
- [X] T034 [P] Run `cargo run --bin mikebom-operator-ctl -- crd --output charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml` and confirm `git diff` is empty (drift check — CRD shape MUST NOT have changed, per constitution VII)
- [X] T035 Run pre-PR gate: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. Confirm all feature 001–006 tests still pass (FR-012, FR-013).
- [X] T036 [P] Run gated E2E locally if kind is available: `MIKEBOM_OPERATOR_E2E=1 cargo test --test reconciler_spawns_job` — all seven new tests (T021, T021b, T027, T028, T030, T031, plus implicit phase-filter coverage) green. Without kind, the gated tests skip cleanly.

---

## Dependencies

```
Phase 1 (Setup: T001 → T002, T003)
  ↓
Phase 2 (Foundational: T004, T005, T006, T007, T008 — all [P] after T003)
  ↓
Phase 3 (US1 MVP):
  T009–T013, T012b [P] (unit tests) ─┐
                                     ├→ T014–T016 [P] (pure helpers)
                                     │    ↓
                                     │  T017, T018 (I/O helpers)
                                     │    ↓
                                     │  T019 (ensure_jobs glue)
                                     │    ↓
                                     │  T020 (reconcile wiring)
                                     │    ↓
                                     └─ T021, T021b (E2E)
  ↓
Phase 4 (US2 status): T022 → T023 → T024 → T025/T026 [P] → T027/T028 [P]
  ↓
Phase 5 (US3 idempotency): T029 → T030/T031 [P]
  ↓
Phase 6 (Polish): T032/T033/T034 [P], T035, T036 [P]
```

**Story independence**: US2 and US3 depend on US1 (the orchestrator must exist before status mapping or idempotency proofs make sense), but US2 and US3 are independent of each other and can land in either order. The MVP cutoff is end of Phase 3.

## Parallel execution opportunities

- Phase 2 (T004–T008): 5 tasks across 2 files (status.rs, scan_orchestrator.rs) — all [P]-eligible after T003.
- Phase 3 unit tests (T009–T013 + T012b): 6 tests in the same module's `#[cfg(test)] mod tests` — file-level conflict only on simultaneous writes; safe in 6-way parallel via test-name uniqueness.
- Phase 3 pure helpers (T014–T016): 3 helpers in the same file — safe as long as they share no signature dependencies (none do, per data-model.md).
- Phase 4 docs (T025, T026): 2 different doc files — fully parallel.
- Phase 4 E2E tests (T027, T028): 2 tests in the same E2E file — sequential file write but independent semantically.
- Phase 6 grep + drift checks (T032–T034, T036): 4 read-only checks across different paths — fully parallel.

## Implementation strategy

**MVP scope**: Phases 1–3. After Phase 3, the operator spawns scan Jobs in the cluster — the "operator actually does something" milestone. Status surface still lies (`NotYetReconciled`) but the underlying behavior is live. A release at end of Phase 3 would be functional but undercommunicated; that's why US2/US3 are P2, not deferred.

**Incremental delivery**:
- After Phase 3: `kubectl get jobs -n kusari-operator` shows spawned Jobs; demonstrable end-to-end.
- After Phase 4: `kubectl get namespacescan -o yaml` shows `reason: Scanning` (or other accurate reason); status surface no longer lies.
- After Phase 5: re-applying the CR is safe; no Job-count explosion across requeue cycles.
- After Phase 6: pre-PR gate passes; ready for fork-based PR to `kusari-oss/mikebom-operator`.

**Test counts to expect** (cumulative, on top of features 001–006's 47 tests):
- Phase 3 unit: +6 (T009–T013 + T012b) → 53 lib tests
- Phase 4 unit: +1 (T022, multiple cases inside one `#[test]`) → 54 lib tests
- Phase 5 unit: +1 (T029) → 55 lib tests
- Phase 3+4+5 gated E2E: +6 (T021, T021b, T027, T028, T030, T031) — all skip cleanly without `MIKEBOM_OPERATOR_E2E=1`.

## Format validation

All 38 tasks follow the format `- [ ] T### [P?] [Story?] Description with file path`. User-story phases (T009–T031, plus T012b and T021b inserted during the /speckit-analyze remediation pass) carry `[US1]`/`[US2]`/`[US3]` labels. Setup, foundational, and polish phases carry no story label. Every task names at least one exact file path under `crates/operator/` or `e2e/` or `docs/`.
