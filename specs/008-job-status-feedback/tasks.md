---

description: "Task list for feature 008: status feedback from Job watch"
---

# Tasks: Status feedback from Job watch

**Input**: Design documents from `/specs/008-job-status-feedback/`

**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/status-aggregator.md, quickstart.md

**Tests**: Included. Same TDD-style discipline as features 001–007 — unit tests inline with implementation modules, integration tests under `e2e/` gated by `MIKEBOM_OPERATOR_E2E=1`. Tests are written first within each phase.

**Organization**: Tasks are grouped by user story. US1 (ScanCompleted) and US2 (ScanFailed) are both P1 and form the "production-credible status surface" pair; US3 (scannedImages population) is P2 — the SBOM manifest layer.

## Format: `[ID] [P?] [Story?] Description with file path`

- **[P]**: Can run in parallel (different files, no in-flight dependency).
- **[Story]**: Required on Phase 3+ tasks (US1 / US2 / US3).

## Path Conventions

Rust workspace with the `operator` crate at `crates/operator/`. The `e2e` crate at `e2e/`. All paths are repo-relative.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Carve out the new module home so later phases have somewhere to land.

- [X] T001 Create empty `status_aggregator` module skeleton at `crates/operator/src/reconcile/status_aggregator.rs` and register it in `crates/operator/src/reconcile/mod.rs`

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Lay down the type system and constants every user story depends on — status reason strings, the new outcome enum, and shared pure helpers used by both ScanCompleted (US1) and ScanFailed (US2) paths.

**⚠️ CRITICAL**: No user story work can begin until this phase is complete.

- [X] T002 [P] Add status reason + status-value constants (`STATUS_TRUE`, `REASON_SCAN_COMPLETED`, `REASON_SCAN_FAILED`) to `crates/operator/src/status.rs` alongside the existing feature 007 constants
- [X] T003 [P] Define `AggregatedOutcome` enum (`AllSucceeded { scanned: Vec<ScannedImage> }`, `AnyFailed { image_ref: String }`, `StillRunning`) per data-model.md in `crates/operator/src/reconcile/status_aggregator.rs`
- [X] T004 [P] Implement pure helper `is_job_succeeded(job: &Job) -> bool` (returns `true` iff `job.status.succeeded >= 1`) in `crates/operator/src/reconcile/status_aggregator.rs`
- [X] T005 [P] Implement pure helper `is_job_finally_failed(job: &Job) -> bool` (returns `true` iff `job.status.failed >= job.spec.backoff_limit.unwrap_or(6) + 1`, per research §6) in `crates/operator/src/reconcile/status_aggregator.rs`
- [X] T006 [P] Implement pure helper `extract_image_ref_from_job(job: &Job) -> Option<String>` (walks `job.spec.template.spec.init_containers[]`, finds the `init-pull` container, returns its `IMAGE_REF` env var value) in `crates/operator/src/reconcile/status_aggregator.rs`

**Checkpoint**: Module skeleton + types exist; user stories can land in parallel.

---

## Phase 3: User Story 1 — Admin sees scan completion (Priority: P1) 🎯 MVP

**Story goal**: When every Job owned by a CR has `.status.succeeded >= 1`, the CR's status transitions to `Ready=True / reason=ScanCompleted` within 5 seconds of the final Job's status update.

**Independent Test**: Install the operator into kind via the chart. Apply a CR + 1 fixture pod with image `X`. Wait for `ensure_jobs` to spawn one Job. Patch the Job's status to `succeeded=1`. Within 5 seconds, `kubectl get namespacescan <name> -o json | jq '.status.conditions[] | select(.type=="Ready") | .status,.reason'` returns `"True"` and `"ScanCompleted"`. (Maps to SC-001, FR-001 through FR-003.)

**Tests first (unit):**

- [X] T007 [P] [US1] Unit test for `aggregate_job_outcomes` with empty `jobs` slice → `StillRunning` (covers the TTL-race row of the decision table) in `crates/operator/src/reconcile/status_aggregator.rs::tests`
- [X] T008 [P] [US1] Unit test for `aggregate_job_outcomes` with all-succeeded fixture (2 Jobs, both with `status.succeeded=1`, each with a distinct `IMAGE_REF` init-pull env var) → `AllSucceeded { scanned }` where `scanned.len() == 2` and each entry has a non-empty `image_ref` in `crates/operator/src/reconcile/status_aggregator.rs::tests`
- [X] T009 [P] [US1] Unit test for `aggregate_job_outcomes` with partial-progress fixture (1 succeeded, 1 still running) → `StillRunning` (succeeded-count < total) in `crates/operator/src/reconcile/status_aggregator.rs::tests`
- [X] T010 [P] [US1] Unit test for `status_with_aggregated_outcome` with `AllSucceeded { scanned: [...] }` base/existing/now → produces `Ready=True, reason=ScanCompleted`, message includes scanned count; `lastTransitionTime` advances when transitioning from `Scanning`, preserved when reason is unchanged across calls — in `crates/operator/src/status.rs::tests`
- [X] T011 [P] [US1] Unit test that the closure `job_to_cr_request` (constructed in `main.rs`-test mode) returns `Some(ObjectRef::new(cr_name).within(operator_namespace))` for a labeled Job, and `None` for an unlabeled Job (FR-011) in `crates/operator/src/main.rs::tests` (or a new `crates/operator/src/watch.rs` if extracted)

**Implementation:**

- [X] T012 [US1] Implement `pub fn aggregate_job_outcomes(jobs: &[Job], spec: &NamespaceScanSpec) -> AggregatedOutcome` per the research §6 decision table — start with the empty-list / partial-progress / all-succeeded rows. **In this phase, the `AllSucceeded` arm returns `scanned: vec![]` (T032 in US3 will extend it to populate the vec via `derive_sbom_location`, which doesn't exist until T030).** AnyFailed arm lands in US2 (T023). — in `crates/operator/src/reconcile/status_aggregator.rs`
- [X] T013 [US1] Implement `pub async fn list_owned_jobs(api: &Api<Job>, cr_name: &str) -> Result<Vec<Job>, kube::Error>` (`ListParams::default().labels(&format!("kusari.dev/namespace-scan={cr_name}"))`) in `crates/operator/src/reconcile/status_aggregator.rs`
- [X] T014 [US1] Implement `pub fn status_with_aggregated_outcome(base, existing, outcome, now) -> NamespaceScanStatus` in `crates/operator/src/status.rs` — landing the `AllSucceeded` and `StillRunning` arms; `AnyFailed` arm lands in US2
- [X] T015 [US1] Extract `operator_namespace` capture-by-clone for the watch mapping closure, build the `job_to_cr_request` mapping fn, and extend the `Controller::new(...)` chain with `.watches(jobs_api, watcher::Config::default(), job_to_cr_request)` in `crates/operator/src/main.rs`
- [X] T016 [US1] Wire `reconcile()` to call `list_owned_jobs(...)` → `aggregate_job_outcomes(...)` → `status_with_aggregated_outcome(...)` on the `OrchestrationResult::Spawned` branch only (FR-009); other branches keep feature 007's `status_with_orchestration_result` path — in `crates/operator/src/reconcile/namespace_scan.rs`
- [X] T016b [US1] Anti-regression grep verification of the FR-009 gating (closes M1 from /speckit-analyze): `grep -c "OrchestrationResult::Spawned" crates/operator/src/reconcile/namespace_scan.rs` returns exactly **1** (the single call site that guards the aggregator); `grep "aggregate_job_outcomes" crates/operator/src/reconcile/namespace_scan.rs` MUST appear inside an `if let OrchestrationResult::Spawned { .. } = ...` block (visual inspection). The static check prevents a refactor from silently moving the aggregator outside the gate — if anyone removes the `if let` guard, the grep count changes. — in `crates/operator/src/reconcile/namespace_scan.rs`

**Tests last (E2E, gated):**

- [X] T017 [US1] Add `e2e/tests/common/mod.rs` (NEW) factoring out the chart-install + image-build + helm-wait scaffolding currently inline in `reconciler_skeleton.rs`. Common namespace/cluster setup, `kubectl_*` helpers, and the install-operator helper become public functions consumable by feature 008's tests. Do NOT modify `reconciler_skeleton.rs` itself in this feature.
- [X] T018 [US1] Add gated kind E2E `t_scan_completed_within_5s` — installs operator, applies CR + 1 fixture pod with image `nginx:1.27.0`, polls until ensure_jobs spawns the Job (using the `kusari.dev/namespace-scan` label), patches the Job's status subresource to `{"status": {"succeeded": 1, "completionTime": <now>}}`, polls CR's status with 5s budget, asserts `Ready=True / reason=ScanCompleted`, in new file `e2e/tests/job_status_feedback.rs`
- [X] T018b [US1] Add gated kind E2E `t_status_survives_operator_restart` (closes H1 from /speckit-analyze; verifies FR-010 + SC-006) — same setup as T018 to reach `Ready=True / reason=ScanCompleted`, then `kubectl delete pod -l app.kubernetes.io/name=mikebom-operator -n kusari-operator` to force the operator to restart. Wait for the new operator pod's Ready condition. Within 30 seconds of the new pod being Ready, the CR's status MUST still report `Ready=True / reason=ScanCompleted` (the watch resync observes the pre-existing Job's terminal state and re-derives the same status). — in `e2e/tests/job_status_feedback.rs`

**Checkpoint**: After Phase 3, `kubectl wait --for=condition=Ready namespacescan/<name>` finally exits 0 on success. The MVP slice ships.

---

## Phase 4: User Story 2 — Admin sees scan failure (Priority: P1)

**Story goal**: When any Job owned by a CR has `.status.failed >= backoffLimit + 1`, the CR's status transitions to `Ready=False / reason=ScanFailed` within 5 seconds, with a message naming the failing image.

**Independent Test**: With Phase 3 landed, apply a CR + 1 pod with image `X`. Patch the Job's status to `{"status": {"failed": <backoffLimit + 1>}}`. Within 5 seconds, `kubectl get namespacescan <name> -o json` returns `reason: ScanFailed`, `message` contains the literal string `X`. (Maps to SC-002, FR-004.)

**Tests first (unit):**

- [X] T019 [P] [US2] Unit test for `aggregate_job_outcomes` with one finally-failed Job (`status.failed=7`, `backoffLimit=6`) → `AnyFailed { image_ref: <X> }` where `<X>` is the failing Job's `IMAGE_REF` in `crates/operator/src/reconcile/status_aggregator.rs::tests`
- [X] T020 [P] [US2] Unit test for `aggregate_job_outcomes` with mixed-state fixture (1 succeeded, 1 finally-failed) → `AnyFailed` (failure dominates partial-success per FR-005) in `crates/operator/src/reconcile/status_aggregator.rs::tests`
- [X] T021 [P] [US2] Unit test for `aggregate_job_outcomes` with retry-in-progress fixture (`status.failed=3`, `backoffLimit=6`) → `StillRunning` (not yet "finally failed") in `crates/operator/src/reconcile/status_aggregator.rs::tests`
- [X] T022 [P] [US2] Unit test for `status_with_aggregated_outcome` `AnyFailed { image_ref }` arm — message MUST contain the literal `image_ref` string (per FR-004) and condition is `Ready=False, reason=ScanFailed` in `crates/operator/src/status.rs::tests`

**Implementation:**

- [X] T023 [US2] Extend `aggregate_job_outcomes` with the `AnyFailed` arm: scan `jobs` for any Job where `is_job_finally_failed(job)`, return the first such Job's `image_ref` (deterministic by label sort order). Failure check happens BEFORE all-succeeded check so failure dominates. — in `crates/operator/src/reconcile/status_aggregator.rs`
- [X] T024 [US2] Extend `status_with_aggregated_outcome` with the `AnyFailed` arm: `Ready=False, reason=ScanFailed, message="scan failed for image \"<image_ref>\""`; `scannedImages[]` passes through unchanged. — in `crates/operator/src/status.rs`

**Tests last (E2E, gated):**

- [X] T025 [US2] Add gated kind E2E `t_scan_failed_within_5s` — same setup as T018, but patches Job status to `{"status": {"failed": 7}}` (default backoffLimit + 1), polls CR's status with 5s budget, asserts `Ready=False / reason=ScanFailed`, message contains the literal image ref — in `e2e/tests/job_status_feedback.rs`
- [X] T026 [US2] Add gated kind E2E `t_failure_dominates_mixed` — applies CR + 2 pods (images `X`, `Y`), waits for 2 Jobs, patches one to succeeded and the other to failed, asserts `reason=ScanFailed` (failure-dominates per FR-005, SC-003) — in `e2e/tests/job_status_feedback.rs`

**Checkpoint**: After Phase 4, both `ScanCompleted` and `ScanFailed` are observable. The CR's status condition tells the *complete* story of the scan outcome.

---

## Phase 5: User Story 3 — Admin sees per-image scan records (Priority: P2)

**Story goal**: After successful scans, `status.scannedImages[]` is populated with one entry per scanned image: `imageRef`, `completedAt`, `sbomLocation` (backend-specific URL).

**Independent Test**: With Phases 3+4 landed, apply a CR with `output.type=Pvc, output.pvc.claimName=test-claim, output.pvc.pathPrefix=team-a` and 2 pods (images `nginx:1.27.0`, `redis:7.4.0`). Wait for ensure_jobs, patch both Jobs to succeeded. Within 5s, `kubectl get namespacescan <name> -o json | jq '.status.scannedImages[]'` returns 2 entries; each `sbomLocation` matches `pvc://test-claim/team-a/<7-hex>.json`. (Maps to SC-004, SC-005, FR-006–FR-008, FR-015.)

**Tests first (unit):**

- [X] T027 [P] [US3] Unit test for `derive_sbom_location` over all 3 backend types: PVC (with and without `pathPrefix`), S3 (with and without `pathPrefix`), OCI. Verify the 3 URL schemes per FR-008 in `crates/operator/src/reconcile/status_aggregator.rs::tests`
- [X] T028 [P] [US3] Unit test for `merge_scanned_images_append_only` — entries are NEVER removed (FR-015); duplicate `image_ref` keys favor newly-completed (newest wins per research §7); output is sorted by `image_ref` (deterministic) in `crates/operator/src/reconcile/status_aggregator.rs::tests`
- [X] T029 [P] [US3] Unit test for `status_with_aggregated_outcome` `AllSucceeded` arm: when `scanned` is populated, the function merges into `status.scannedImages[]` and advances `status.lastScanCompletedAt` to `max(scanned.completedAt)` in `crates/operator/src/status.rs::tests`

**Implementation:**

- [X] T030 [P] [US3] Implement pure helper `fn derive_sbom_location(spec: &NamespaceScanSpec, short_hash: &str) -> String` per contract — three arms (PVC/S3/OCI), each respecting the optional `pathPrefix` and producing the URL shape from FR-008 — in `crates/operator/src/reconcile/status_aggregator.rs`
- [X] T031 [P] [US3] Implement pure helper `fn merge_scanned_images_append_only(existing: &[ScannedImage], newly_completed: Vec<ScannedImage>) -> Vec<ScannedImage>` per research §7 (BTreeMap-by-image_ref, newest wins, never removes) in `crates/operator/src/reconcile/status_aggregator.rs`
- [X] T032 [US3] Extend `aggregate_job_outcomes`'s `AllSucceeded` arm to populate `scanned: Vec<ScannedImage>` — for each succeeded Job, extract its `IMAGE_REF` (via `extract_image_ref_from_job`), pull its `kusari.dev/image-ref-hash` label, build `ScannedImage { image_ref, resolved_sha: None, sbom_location: derive_sbom_location(...), completed_at: status.completion_time }` — in `crates/operator/src/reconcile/status_aggregator.rs`
- [X] T033 [US3] Extend `status_with_aggregated_outcome`'s `AllSucceeded` arm to: (a) call `merge_scanned_images_append_only` with `existing.scannedImages` + the outcome's `scanned`, (b) write the merged vec into the new status's `scannedImages`, (c) advance `lastScanCompletedAt` to `max(merged.completedAt)` — in `crates/operator/src/status.rs`

**Tests last (E2E, gated):**

- [X] T034 [US3] Add gated kind E2E `t_scanned_images_populated_pvc` — applies CR with PVC output + 2 fixture pods (distinct images), patches both Jobs to succeeded, asserts `status.scannedImages[]` has 2 entries with `imageRef` non-empty, `sbomLocation` matching `pvc://<claim>/<prefix>/<7-hex>.json`, `completedAt` parseable as RFC 3339 — in `e2e/tests/job_status_feedback.rs`
- [X] T034b [US3] Add gated kind E2E `t_scanned_images_append_only_across_edits` (closes M2 from /speckit-analyze; verifies SC-005 + FR-015 end-to-end) — same setup as T034, then (a) `kubectl patch namespacescan <name> --type merge -p '{"spec":{"output":{"pvc":{"pathPrefix":"team-b"}}}}'` to change the path prefix mid-flight, (b) `kubectl apply` a third fixture pod with a distinct image, (c) wait for the new Job to appear and patch it to succeeded, (d) assert `status.scannedImages[]` now has 3 entries, AND the original 2 entries' `sbomLocation` strings are UNCHANGED (the URL the operator derives at status-aggregation time will use the new `team-b` prefix per research §5's documented caveat, but the *previously-written* entries stay put per FR-015's append-only invariant). — in `e2e/tests/job_status_feedback.rs`

**Checkpoint**: After Phase 5, admins can enumerate SBOM artifacts via the CR's status. Downstream tooling can consume the `scannedImages[]` manifest.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Pre-PR gate, anti-regression, and the static checks the constitution requires.

- [X] T035 [P] Grep verification that no SBOM-content parsing happens in `crates/operator/src/reconcile/status_aggregator.rs` — fail if any of `serde_json::from_str`, `cyclonedx`, `spdx`, or `fs::read.*workdir/out` appears (Constitution II, FR — no SBOM access)
- [X] T036 [P] Grep verification that no `api.delete(.*job.*)` or `Api::<Job>.*delete` happens in `crates/operator/src/reconcile/` (FR-014 — no active Job deletion; cleanup is `ttlSecondsAfterFinished` + ownerRef GC)
- [X] T037 [P] Run `cargo run --bin mikebom-operator-ctl -- crd --output charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml` and confirm `git diff` is empty (drift check — CRD shape MUST NOT have changed, per constitution VII)
- [X] T038 Run pre-PR gate: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. Confirm all feature 001–007 tests still pass (FR-012, FR-013).
- [X] T039 [P] Update `docs/architecture.md` — move `ScanCompleted` (with `status=True`) and `ScanFailed` from the "reserved for future features" table into the "values written by the operator" table; reserved table now empties out (or carries placeholder for feature 009+ if any reasons emerge)
- [X] T040 [P] Update `docs/crd-reference.md` — promote `ScanCompleted` and `ScanFailed` rows from reserved-for-future to active; add notes on the `kubectl wait --for=condition=Ready` ergonomic
- [X] T041 [P] Run gated E2E locally if kind is available: `MIKEBOM_OPERATOR_E2E=1 cargo test --test job_status_feedback` — all 4 new tests (T018, T025, T026, T034) green. Without kind, the gated tests skip cleanly.

---

## Dependencies

```
Phase 1 (Setup: T001)
  ↓
Phase 2 (Foundational: T002–T006 — all [P] after T001)
  ↓
Phase 3 (US1 MVP):
  T007–T011 [P] (unit tests) ─┐
                              ├→ T012 (aggregator AllSucceeded+StillRunning, scanned=vec![])
                              │    ↓
                              │  T013 (list_owned_jobs)
                              │    ↓
                              │  T014 (status mapper AllSucceeded+StillRunning)
                              │    ↓
                              │  T015 (watch wiring in main.rs)
                              │    ↓
                              │  T016 (reconcile wiring)
                              │    ↓
                              │  T016b (grep check FR-009 gating)
                              │    ↓
                              └─ T017 (e2e common scaffolding)
                                   ↓
                                 T018 (E2E ScanCompleted), T018b (E2E restart resync) [P]
  ↓
Phase 4 (US2 failure):
  T019–T022 [P] (unit tests) → T023 (AnyFailed arm) → T024 (status mapper AnyFailed) → T025/T026 [P] (E2E)
  ↓
Phase 5 (US3 scannedImages):
  T027–T029 [P] (unit tests) → T030/T031 [P] (pure helpers) → T032 (aggregator populates scanned) → T033 (status mapper writes scannedImages[]) → T034/T034b [P] (E2E)
  ↓
Phase 6 (Polish): T035/T036/T037/T039/T040/T041 [P], T038 (sequential gate)
```

**Story independence**: US2 depends on US1 (both modify the same `aggregate_job_outcomes` function — sequential file edit), US3 depends on both (extends the `AllSucceeded` arm's data shape). MVP is end of Phase 3.

## Parallel execution opportunities

- Phase 2 (T002–T006): 5 tasks across 2 files — T002 in `status.rs`, T003–T006 in `status_aggregator.rs`. T003–T006 share a file so [P] denotes "logically independent" rather than "concurrent file write."
- Phase 3 unit tests (T007–T011): 5 tests across 3 files (`status_aggregator.rs::tests`, `status.rs::tests`, `main.rs::tests`).
- Phase 4 unit tests (T019–T022): 4 tests in `status_aggregator.rs::tests` + `status.rs::tests`.
- Phase 5 unit tests (T027–T029): 3 tests across 2 files.
- Phase 5 pure helpers (T030, T031): 2 helpers in the same file — file-level conflict only on simultaneous writes; safe in 2-way logical parallel.
- Phase 6 (T035–T037, T039–T041): 6 read-only or doc tasks across multiple files — fully parallel except T038 (sequential pre-PR gate).

## Implementation strategy

**MVP scope**: Phases 1–3 (end of T018). After Phase 3, `kubectl wait --for=condition=Ready namespacescan/<name>` works for the happy path. The status condition transitions to `ScanCompleted` correctly. Failures (US2) and the per-image manifest (US3) are not yet observable — admin still has to inspect Job objects for failures, but the success path is fully ergonomic.

**Incremental delivery**:
- After Phase 3: success path is live; `kubectl wait` works on happy path; failures still surface as `Scanning` indefinitely.
- After Phase 4: failures surface as `ScanFailed` with the image in the message. Status condition tells the complete outcome story.
- After Phase 5: `status.scannedImages[]` populated; downstream tooling can consume the SBOM manifest. The full feature 008 spec is delivered.
- After Phase 6: pre-PR gate passes; ready for fork-based PR.

**Test counts to expect** (cumulative, on top of features 001–007's 72 lib + 2 drift + 15 E2E):
- Phase 3 unit: +5 (T007–T011) → 77 lib tests
- Phase 4 unit: +4 (T019–T022) → 81 lib tests
- Phase 5 unit: +3 (T027–T029) → 84 lib tests
- Phase 3+4+5 gated E2E: +6 (T018, T018b, T025, T026, T034, T034b) — all skip cleanly without `MIKEBOM_OPERATOR_E2E=1`.

## Format validation

All 44 tasks follow the format `- [ ] T### [P?] [Story?] Description with file path`. User-story phases (T007–T034b, plus T016b/T018b/T034b inserted during the /speckit-analyze remediation pass) carry `[US1]`/`[US2]`/`[US3]` labels. Setup, foundational, and polish phases carry no story label. Every task names at least one exact file path under `crates/operator/`, `e2e/`, or `docs/`.
