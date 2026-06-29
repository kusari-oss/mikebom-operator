# Implementation Plan: Status feedback from Job watch

**Branch**: `008-job-status-feedback` | **Date**: 2026-06-28 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/008-job-status-feedback/spec.md`

## Summary

Wire kube-rs's `Controller::watches(Job, ...)` so the operator reacts to scan-Job transitions. When a Job's status changes, the mapping fn extracts the owning CR's namespace+name from the `kusari.dev/namespace-scan` label and enqueues a reconcile. Inside `reconcile()`, after feature 007's `ensure_jobs` returns `Spawned`, a new pure `aggregate_job_outcomes` function inspects every Job's `.status.{succeeded,failed,completionTime}` fields and computes one of three outcomes (`AllSucceeded { scanned }`, `AnyFailed { image_ref }`, `StillRunning`). The outcome flows into a new `status_with_aggregated_outcome` mapper that produces the final `Ready` condition (`ScanCompleted=True`, `ScanFailed=False`, or preserves feature 007's `Scanning=False`) and merges newly-completed scans into `status.scannedImages[]` append-only. `imageRef` lookup reads the `IMAGE_REF` env var from the Job's `init-pull` container (already set by feature 003's builder — no builder change needed). `sbomLocation` is derived from the CR's `output` block + the Job's short-image-hash label, keeping the spec's "Job-at-spawn-time, not current CR" invariant. No CRD shape changes; no Helm chart RBAC changes; the existing `Controller` builder already imports `kube::runtime::watcher`, so the new `.watches(jobs_api, ...)` line is a small extension. The new E2E is the first in this repo to exercise watch behavior, so it requires the existing chart-install + operator-image-load scaffolding (the same pattern feature 002's `reconciler_skeleton.rs` already uses).

## Technical Context

**Language/Version**: Rust 1.85+ stable (workspace toolchain, same as features 001–007).

**Primary Dependencies**:
- `kube` 0.97 — `Controller::watches(Api<Job>, watcher::Config, mapper_fn)`, `Api::<Job>::namespaced(...).list(&ListParams::default().labels(...))`. Already in workspace.
- `k8s-openapi` 0.23 with `v1_31` feature — `batch::v1::{Job, JobStatus}`, `core::v1::EnvVar`. Already in workspace.
- No new direct deps.

**Storage**: N/A — reads Job status fields, writes CR status fields. No persistent storage.

**Testing**:
- **Unit tests** in `crates/operator/src/reconcile/status_aggregator.rs`: pure-function tests for `aggregate_job_outcomes` over all 9 cells of the decision table (all-succeeded × N, any-failed × N, partial-progress × N), plus `extract_image_ref_from_job` (with/without init-pull container), `derive_sbom_location` (all 3 backends), `merge_scanned_images_append_only` (dedup-by-imageRef edge cases).
- **Unit tests** in `crates/operator/src/status.rs`: `status_with_aggregated_outcome` decision-table mapping per the new entries in research.md §8.
- **Integration test** (new `e2e/tests/job_status_feedback.rs`, gated by `MIKEBOM_OPERATOR_E2E=1`): runs a real operator pod in kind via the existing chart-install scaffolding from `reconciler_skeleton.rs`, applies a fixture CR + fixture pods, patches owned Job status to `succeeded=1` or `failed=N+1`, polls the CR's status for the expected transition within the 5s/30s budgets, asserts `scannedImages[]` shape.
- Existing tests stay green; in-process E2E from feature 007 is not modified.

**Target Platform**: Linux x86_64 / macOS dev — same as features 001–007.

**Project Type**: Rust workspace — implementation lives in the existing `operator` crate, mostly in a new `reconcile/status_aggregator.rs` module.

**Performance Goals**:
- Watch event delivery ≤ ~300ms (kube-rs typical).
- Reconcile + aggregate + patch ≤ 1s.
- SC-001/SC-002's 5s budget accommodates watch lag + reconcile + patch + headroom.

**Constraints**:
- All feature 001–007 tests MUST continue to pass (FR-012, FR-013). The new `Ctx` carries no additional fields; the new aggregator is called *after* `ensure_jobs` returns `Spawned`, so the `InvalidSpec` / `NoImagesInScope` / `BuildFailed` / `RBACInsufficient` paths are untouched.
- Constitution II (USE not EMBED): the aggregator MUST NOT parse SBOM contents. It only reads Job `.status.{succeeded,failed,completionTime}` and the Job's init-pull env var for `imageRef`.
- Constitution IV (CRD Backward Compat): no schema changes. Feature 002's `ScannedImage` struct (`imageRef`, `resolvedSha`, `sbomLocation`, `completedAt`) is populated for the first time; the field shape stays the same.

**Scale/Scope**:
- v0.8 target: ≤25 distinct Jobs per CR (feature 007's scale), ≤25 `scannedImages[]` entries per CR. Cross-CR concurrency bounded by kube-rs's default Controller concurrency.
- Watch traffic: O(Job transitions per cluster). With 100 CRs × 25 images, the upper bound is ~2.5k Jobs cluster-wide; watch event volume is dominated by Job phases (Pending → Running → Completed) plus retries on `backoffLimit` failure — well within kube-rs / apiserver budgets for v0.x.

## Constitution Check

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Pure Rust where reasonable | PASS | No new deps; existing pure-Rust stack. |
| II. USE not EMBED (NON-NEGOTIABLE) | PASS | Aggregator reads `Job.status` + Job spec env vars only. No SBOM-content parsing; the operator never reads `/workdir/out/*.json`. |
| III. Fail Closed on RBAC (NON-NEGOTIABLE) | PASS | No new RBAC verbs needed (`batch/v1.jobs:watch` already granted cluster-wide). If watch establishment fails (e.g., 403 because someone tightened RBAC), kube-rs's Controller surfaces a runtime error via the `controller_runtime_error` log path already in `main.rs`. |
| IV. CRD Backward Compatibility | PASS | Zero schema changes. `ScannedImage` struct from feature 002 gets populated for the first time; field shapes are unchanged. Two new condition reasons (`ScanCompleted`, `ScanFailed`) are reserved already in `docs/architecture.md` and additive to the v1alpha1 `Ready` vocabulary. |
| V. SBOM-Format Agnostic | PASS | No SBOM parsing. `sbomLocation` is a URL string derived from the CR's `output` block + the Job's short-image-hash, never an SBOM parse. |
| VI. Hermetic E2E Tests (NON-NEGOTIABLE) | PASS | New gated E2E exercises the watch-driven status path end-to-end in kind. This is the first feature whose behavior can't be unit-tested in-process — watch reactivity is a runtime property. The test reuses feature 002's chart-install + image-load + helm-wait scaffolding. |
| VII. Helm Chart Lockstep | PASS | No CRD shape changes (drift check stays green). No chart changes (RBAC already covers `jobs:get,list,watch`). |

All gates pass. No `## Complexity Tracking` section needed.

## Project Structure

### Documentation (this feature)

```text
specs/008-job-status-feedback/
├── plan.md                              # this file
├── spec.md                              # spec (no clarifications were needed)
├── research.md                          # Phase 0: 9 decisions (watch wiring, mapper fn, label-based CR lookup, image-ref source, sbomLocation derivation, aggregation decision table, append-only merge semantics, condition reason constants, kind E2E scaffolding)
├── data-model.md                        # Phase 1: types added (AggregatedOutcome, ScannedImage population), FR→test mapping
├── quickstart.md                        # Phase 1: admin upgrade flow (no chart edits) + how `kubectl wait --for=condition=Ready` finally works
├── contracts/
│   └── status-aggregator.md             # Internal contract: aggregate_job_outcomes + status_with_aggregated_outcome
└── tasks.md                             # /speckit-tasks output (not created here)
```

### Source Code (repository root)

```text
crates/operator/src/
├── main.rs                              # MODIFY (small):
│                                        #   - Extend `Controller::new(api, watcher::Config::default())`
│                                        #     with `.watches(jobs_api, watcher::Config::default(), job_to_cr_request)`
│                                        #   - Add `job_to_cr_request` mapping fn (translates Job event → optional ObjectRef<NamespaceScan>)
│
├── reconcile/
│   ├── mod.rs                           # MODIFY: register new `status_aggregator` submodule
│   ├── namespace_scan.rs                # MODIFY:
│   │                                    #   - After `ensure_jobs` returns `Spawned`, call
│   │                                    #     `status_aggregator::list_owned_jobs(...)` then
│   │                                    #     `status_aggregator::aggregate_job_outcomes(...)` then
│   │                                    #     `status::status_with_aggregated_outcome(...)`
│   │                                    #   - When result is `NoImagesInScope`/`BuildFailed`/`RbacInsufficient`,
│   │                                    #     skip aggregation (FR-009)
│   ├── scan_orchestrator.rs             # UNCHANGED (feature 007's contract is stable; the aggregator is a peer)
│   └── status_aggregator.rs             # NEW:
│                                        #   - `pub enum AggregatedOutcome { AllSucceeded { scanned: Vec<ScannedImage> }, AnyFailed { image_ref: String }, StillRunning }`
│                                        #   - `pub fn aggregate_job_outcomes(jobs: &[Job], spec: &NamespaceScanSpec) -> AggregatedOutcome`
│                                        #   - Pure helpers (unit-tested):
│                                        #       * `is_job_succeeded(job: &Job) -> bool` (status.succeeded >= 1)
│                                        #       * `is_job_finally_failed(job: &Job) -> bool` (status.failed >= job.spec.backoffLimit + 1)
│                                        #       * `extract_image_ref_from_job(job: &Job) -> Option<String>` (reads init-pull container's IMAGE_REF env var per research §4)
│                                        #       * `derive_sbom_location(spec: &NamespaceScanSpec, short_hash: &str) -> String` (PVC/S3/OCI URL per FR-008)
│                                        #       * `merge_scanned_images_append_only(existing: &[ScannedImage], newly_completed: Vec<ScannedImage>) -> Vec<ScannedImage>` (per FR-015)
│                                        #   - I/O helper (integration-tested via E2E):
│                                        #       * `list_owned_jobs(api: &Api<Job>, cr_name: &str) -> Result<Vec<Job>, kube::Error>` (label-selector list)
│
├── scan_job/mod.rs                      # UNCHANGED. Existing `kusari.dev/namespace-scan` + `kusari.dev/image-ref-hash` labels + IMAGE_REF env var on init-pull are sufficient; no builder change.
│
└── status.rs                            # MODIFY (small):
                                         #   - Add condition reason constants `REASON_SCAN_COMPLETED` and `REASON_SCAN_FAILED`
                                         #   - Add `STATUS_TRUE: &str = "True"` (companion to existing STATUS_FALSE)
                                         #   - Add `pub fn status_with_aggregated_outcome(base: NamespaceScanStatus, existing: Option<&NamespaceScanStatus>, outcome: &AggregatedOutcome, now: DateTime<Utc>) -> NamespaceScanStatus`
                                         #   - The function:
                                         #       * AllSucceeded → Ready=True / reason=ScanCompleted
                                         #         + merge `scanned` into status.scannedImages[]
                                         #         + advance status.lastScanCompletedAt to max(completedAt)
                                         #       * AnyFailed{image_ref} → Ready=False / reason=ScanFailed
                                         #       * StillRunning → preserve `base` (which carries Scanning from feature 007)

e2e/tests/
├── job_status_feedback.rs               # NEW (gated): real-operator-in-kind E2E
│                                        #   - Reuses feature 002's chart-install + image-build + helm-wait scaffolding
│                                        #     (extracted into a shared helper module if needed, or copy-paste for v0.8)
│                                        #   - 3 tests: T-success (one Job → ScanCompleted), T-failure (Job retries
│                                        #     exhausted → ScanFailed), T-mixed (1 succeeded + 1 failed → ScanFailed
│                                        #     dominates)
│                                        #   - Polls status.conditions[Ready] with a tight (5s) budget
│                                        #   - Asserts status.scannedImages[] is populated with PVC sbomLocation
└── reconciler_spawns_job.rs             # UNCHANGED (in-process orchestrator tests don't touch watch)

charts/mikebom-operator/
└── (no changes)                         # CRD YAML drift check stays green; RBAC already grants the watch verb.

docs/
├── architecture.md                      # MODIFY: move `ScanCompleted` (status=True) and `ScanFailed` from "reserved" to "written by feature 008"
└── crd-reference.md                     # MODIFY: same — promote ScanCompleted/ScanFailed rows from reserved to active
```

**Structure Decision**:

The aggregator gets its own sibling under `reconcile/` (peer to `scan_orchestrator.rs` from feature 007), not folded into the orchestrator. Rationale:

- **Single responsibility**: orchestration *spawns* Jobs; aggregation *interprets* them. Feature 007's contract explicitly lists "reading Job status" as a non-goal, and merging the two would break that boundary.
- **Independent testability**: aggregation is fully pure (`Job` → `AggregatedOutcome` → status); orchestration has I/O (pod list + Job create). Keeping them separate keeps the unit tests honest.
- **Future-proofing**: feature 009 (schedule honoring) will need to *delete* completed Jobs to re-scan. That deletion logic naturally lives next to aggregation, not orchestration.

The status mapper (`status_with_aggregated_outcome`) lives in `status.rs` alongside feature 007's `status_with_orchestration_result` — both are "status decision table" functions and belong together.

## Phase 0: Outline & Research

Research artifact: [research.md](./research.md). The 9 decisions it records:

1. **Watch wiring**: `Controller::new(api, watcher::Config::default()).watches(jobs_api, watcher::Config::default(), job_to_cr_request)`. The `jobs_api` is `Api::<Job>::namespaced(client, &operator_namespace)` (filters to operator's namespace at the source, before the mapper fn runs). Alternative considered: label-selector scoping inside `watcher::Config` (deferred — namespace-scoping is sufficient for v0.8 since all spawned Jobs live in the operator's namespace).

2. **Mapping fn shape**: `fn job_to_cr_request(job: Arc<Job>) -> Option<ObjectRef<NamespaceScan>>`. Returns `None` for Jobs without the `kusari.dev/namespace-scan` label (FR-011). Returns `Some(ObjectRef::new(cr_name).within(operator_namespace))` for owned Jobs. The label is the source of truth for CR identification (not `ownerReferences`, since label lookup is O(1) on a single Job and ownership-ref iteration is O(N) per event).

3. **Operator namespace in mapper**: the mapper fn captures `operator_namespace: String` by clone via the `move` keyword on the closure. Since the Controller's mapper-fn signature is `Fn(Arc<Job>) -> impl IntoIterator<Item=ObjectRef<NamespaceScan>>`, the closure builds the `ObjectRef` with the captured namespace string.

4. **`imageRef` source**: read `IMAGE_REF` from the Job's `init-pull` container's env var. Feature 003 already sets this; no builder change needed. Alternative (new `kusari.dev/image-ref` label) considered and rejected — image refs can contain `:` and `/` which need label-value sanitization (max 63 chars, no `:` / `/`), and the env-var path is already present without modification.

5. **`sbomLocation` derivation**: a single pure helper `derive_sbom_location(spec, short_hash) -> String` produces the three URL schemes per FR-008. The function reads `spec.output.backend_type` + the matching `spec.output.{pvc,s3,oci}` block. The `<ext>` token is always `.json` for v0.7's three scan formats (CDX-JSON, SPDX-2.3-JSON, SPDX-3-JSON). Source of truth for the Job's backend is the *CR's spec* at status-aggregation time — which is the same as the Job's spawn-time backend in 99% of cases. The spec's Edge Case "CR `output` edited mid-scan" calls out that previously-spawned Jobs continue with the *old* backend, so `sbomLocation` derived from the *current* CR spec could disagree with where the Job *actually* wrote. **Decision**: accept this risk for v0.8 (the edge case requires admin action mid-scan) and document the limitation; a future feature can read the actual backend from the Job's output-upload container env if needed. Alternative considered (read from Job spec): rejected as too much code for an edge case.

6. **Aggregation decision table** (drives `aggregate_job_outcomes`):

   | Owned Jobs (filtered by `kusari.dev/namespace-scan` label) | Outcome |
   |---|---|
   | Empty list (zero Jobs found despite `ensure_jobs` returning `Spawned`) | `StillRunning` (default — next reconcile re-creates per feature 007's idempotency; avoids ScanCompleted flapping) |
   | ≥1 Job with `.status.failed >= .spec.backoffLimit + 1` | `AnyFailed { image_ref }` (first failing image; failure dominates) |
   | All Jobs have `.status.succeeded >= 1` | `AllSucceeded { scanned: Vec<ScannedImage> }` |
   | Otherwise (some still pending, some succeeded, none failed) | `StillRunning` |

   The "Empty list" row covers the narrow window where TTL fires between create and list. The "AnyFailed" row covers FR-005's failure-dominates rule. The "AllSucceeded" row is the happy path. The "Otherwise" row is the mixed-progress steady state.

7. **Append-only merge of `scannedImages[]`** (FR-015):

   ```rust
   fn merge_scanned_images_append_only(
       existing: &[ScannedImage],
       newly_completed: Vec<ScannedImage>,
   ) -> Vec<ScannedImage>
   ```

   Algorithm: walk `existing`; for each entry, if its `imageRef` is in `newly_completed`, prefer the newly-completed copy (advances `completedAt`). For each entry in `newly_completed` not in `existing`, append. The final list is sorted by `imageRef` for deterministic output. **No removal**: an entry whose image is no longer in scope stays in the array (FR-015). Alternative considered: pure-append + dedup-by-name later (rejected — duplicate keys would surprise downstream `jq` users).

8. **Condition reason + status constants** (in `crate::status`):

   ```rust
   pub const STATUS_TRUE: &str = "True";          // NEW companion to existing STATUS_FALSE
   pub const REASON_SCAN_COMPLETED: &str = "ScanCompleted";  // NEW
   pub const REASON_SCAN_FAILED: &str = "ScanFailed";        // NEW
   ```

   Plus the new mapper:

   ```rust
   pub fn status_with_aggregated_outcome(
       base: NamespaceScanStatus,
       existing: Option<&NamespaceScanStatus>,
       outcome: &AggregatedOutcome,
       now: DateTime<Utc>,
   ) -> NamespaceScanStatus
   ```

   Mapping:
   - `AllSucceeded { scanned }` → `Ready=True, reason=ScanCompleted`; merge `scanned` into status.scannedImages[]; advance `last_scan_completed_at = max(scanned.completedAt)`.
   - `AnyFailed { image_ref }` → `Ready=False, reason=ScanFailed`; status.scannedImages[] passes through unchanged.
   - `StillRunning` → preserve `base` (which carries Scanning from feature 007's mapper).

   `lastTransitionTime` follows the same idempotency rule as feature 007's mapper: preserved when (status, reason) is unchanged on the prior `existing` condition.

9. **Kind E2E scaffolding**: the new `e2e/tests/job_status_feedback.rs` reuses the chart-install + image-build + helm-wait pattern from `reconciler_skeleton.rs`. Because the test needs three separate scenarios (success, failure, mixed) each requiring a fresh operator install, we factor the shared scaffolding into a small `e2e/tests/common/mod.rs` (or `e2e/src/lib.rs` shared module). Alternatives considered: spawning a real Job and waiting ~5 minutes for natural completion (rejected — flaky and slow); skipping the failure case entirely (rejected — US2 is P1).

**Output**: research.md with all 9 decisions resolved. No `NEEDS CLARIFICATION` markers remain.

## Phase 1: Design & Contracts

**Prerequisites**: research.md complete.

### Data model

[data-model.md](./data-model.md) captures:

- **`AggregatedOutcome` (new)**:
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum AggregatedOutcome {
      AllSucceeded { scanned: Vec<ScannedImage> },
      AnyFailed { image_ref: String },
      StillRunning,
  }
  ```
  Not `#[non_exhaustive]` — internal to the `operator` crate.

- **`ScannedImage` (existing, now populated by feature 008)**:
  - `image_ref`: populated from init-pull's `IMAGE_REF` env var.
  - `resolved_sha`: stays `None` in v0.8 (deferred per Assumptions).
  - `sbom_location`: backend-specific URL per FR-008.
  - `completed_at`: Job's `status.completionTime` as RFC 3339.

- **`Ctx` (unchanged)**: the new mapping fn captures `operator_namespace` separately at `main.rs` Controller setup time.

- **Status reason constants (new in `status.rs`)**: `STATUS_TRUE`, `REASON_SCAN_COMPLETED`, `REASON_SCAN_FAILED`. The existing `STATUS_FALSE` / `REASON_*` from features 002/007 are unchanged.

- **FR → test mapping**:
  - FR-001 (Job watch) → integration test patches Job status and observes reconcile-triggered CR status update; the watch wiring is exercised end-to-end.
  - FR-002 (list owned Jobs) → integration test asserts the label-selector path returns the right set; unit test covers `merge_scanned_images_append_only`.
  - FR-003 (ScanCompleted) → unit test for `aggregate_job_outcomes` + `status_with_aggregated_outcome`; integration test asserts the CR condition flips to `Ready=True`.
  - FR-004 (ScanFailed with image_ref in message) → unit test + integration test asserts the failing image's ref appears in the message.
  - FR-005 (aggregation order: failure > running > succeeded) → unit tests parameterized over all 4 cells of the decision table.
  - FR-006/FR-007/FR-008 (`scannedImages[]` population) → unit tests + integration test asserts `sbomLocation` format for the PVC backend.
  - FR-009 (don't override NoImagesInScope/BuildFailed/RbacInsufficient) → unit test asserts that when feature 007 returned a non-`Spawned` result, the aggregator is not called.
  - FR-010 (operator restart resync) → integration test: install operator, run a CR through to ScanCompleted, restart operator pod, assert status still reads ScanCompleted after restart.
  - FR-011 (ignore unowned events) → unit test: `job_to_cr_request(job_without_label)` returns `None`.
  - FR-012/FR-013 (feature 007/002 untouched) → existing tests stay green.
  - FR-014/FR-015 (no Job delete, append-only) → unit tests for `merge_scanned_images_append_only`.

### Contracts

[contracts/status-aggregator.md](./contracts/status-aggregator.md) — internal contract for `aggregate_job_outcomes` + `status_with_aggregated_outcome`. Pins:

- Pure-function signatures, no I/O.
- Decision-table invariants (failure dominates partial-success, empty list defaults to StillRunning).
- Append-only invariant on `scannedImages[]`.
- The `imageRef`-from-Job extraction contract (init-pull container's `IMAGE_REF` env var; missing → that Job contributes nothing to `scanned`, but still counts for aggregation).

### Agent context update

The project's `CLAUDE.md` currently has `Active plan: [specs/007-reconciler-spawns-job/plan.md](specs/007-reconciler-spawns-job/plan.md)`. Phase 1 updates this to point at the new plan.

**Output**: data-model.md, contracts/status-aggregator.md, quickstart.md, updated `CLAUDE.md`.

## Re-evaluate Constitution Check (post-design)

| Principle | Status | Notes |
|-----------|--------|-------|
| I | PASS | Confirmed: zero new direct deps. |
| II | PASS | Confirmed: aggregator reads Job `.status` + Job spec env vars only. No SBOM content access. |
| III | PASS | Confirmed: no new RBAC verbs. The `kube::runtime::watcher` already inherits the operator's existing `jobs:watch` grant. |
| IV | PASS | Confirmed: zero CRD shape changes. The two new reasons (`ScanCompleted`, `ScanFailed`) are reserved per `docs/architecture.md` and additive. |
| V | PASS | Confirmed: no SBOM access; `sbomLocation` is a URL string derived from the CR's `output` block, not an SBOM parse. |
| VI | PASS | Confirmed: the new E2E covers the watch-driven behavior the in-process integration tests can't exercise. |
| VII | PASS | Confirmed: no CRD shape change; no chart change. Drift check stays green. |

All gates still pass post-design. No complexity tracking needed.
