# Phase 1 Data Model: Status feedback from Job watch

Records the Rust types added/modified by feature 008 and the FR → test mapping.
No CRD shape changes; all types here are internal to the `operator` crate or
populate existing `v1alpha1` fields (`ScannedImage`, `lastScanCompletedAt`).

## New types

### `crate::reconcile::status_aggregator::AggregatedOutcome`

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AggregatedOutcome {
    /// Every owned Job has `status.succeeded >= 1`. `scanned` carries one
    /// `ScannedImage` per Job — derived from the Job's IMAGE_REF env var,
    /// status.completionTime, and the CR's output backend.
    AllSucceeded { scanned: Vec<ScannedImage> },

    /// At least one owned Job has `status.failed >= backoffLimit + 1`. The
    /// `image_ref` field carries the first failing image (deterministic by
    /// label sort order). Failure dominates partial-success.
    AnyFailed { image_ref: String },

    /// Default: empty Job list, or some Jobs still running/retrying within
    /// budget. Preserves whatever reason `base` already has (Scanning from
    /// feature 007).
    StillRunning,
}
```

Not `#[non_exhaustive]` — internal to the `operator` crate.

## Modified types

### `crate::crds::namespace_scan::ScannedImage` (populated for the first time)

```rust
pub struct ScannedImage {
    pub image_ref: String,             // from init-pull IMAGE_REF env var
    pub resolved_sha: Option<String>,  // None in v0.8 (deferred per Assumptions)
    pub sbom_location: String,         // backend-specific URL per FR-008
    pub completed_at: String,          // Job.status.completionTime as RFC 3339
}
```

The struct was added in feature 002 (CRD schema). Feature 008 is the first
caller that writes to it. No field shape change → constitution IV preserved.

### Status reason constants (in `crate::status`)

```rust
pub const STATUS_FALSE: &str = "False";          // existing
pub const STATUS_TRUE: &str = "True";            // NEW
pub const REASON_NOT_YET_RECONCILED: &str = …;   // existing (feat 002)
pub const REASON_INVALID_SPEC: &str = …;         // existing (feat 002)
pub const REASON_SCANNING: &str = …;             // existing (feat 007)
pub const REASON_NO_IMAGES_IN_SCOPE: &str = …;   // existing (feat 007)
pub const REASON_RBAC_INSUFFICIENT: &str = …;    // existing (feat 007)
pub const REASON_BUILD_FAILED: &str = …;         // existing (feat 007)
pub const REASON_SCAN_COMPLETED: &str = "ScanCompleted";  // NEW (feat 008)
pub const REASON_SCAN_FAILED: &str = "ScanFailed";        // NEW (feat 008)
```

### `crate::status::status_with_aggregated_outcome` (new)

```rust
pub fn status_with_aggregated_outcome(
    base: NamespaceScanStatus,
    existing: Option<&NamespaceScanStatus>,
    outcome: &AggregatedOutcome,
    now: DateTime<Utc>,
) -> NamespaceScanStatus
```

Pure function. Implements the decision table:

| `outcome` | Resulting condition | Side effects |
|---|---|---|
| `AllSucceeded { scanned }` | `Ready=True, reason=ScanCompleted, message="scanned N images: ..."` | Merge `scanned` into `status.scannedImages[]` via append-only; advance `status.lastScanCompletedAt` to `max(scanned.completedAt)` |
| `AnyFailed { image_ref }` | `Ready=False, reason=ScanFailed, message="scan failed for image \"<ref>\""` | `scannedImages[]` passes through unchanged |
| `StillRunning` | preserve `base` (Scanning from feature 007's mapper) | passes through unchanged |

`lastTransitionTime` follows the feature 002 idempotency rule: preserved when
`(status, reason)` matches the prior `existing.conditions[Ready]`, advanced
otherwise. (Same code shape as `status_with_orchestration_result`.)

## Wire-level flow

For a single CR reconcile cycle in v0.8:

```
desired_status(spec, now, existing)         # feature 002
   │
   │ base_reason ∈ {InvalidSpec, NotYetReconciled}
   │
   ├─ InvalidSpec ────────────────▶ patch & return (feature 002 short-circuit)
   │
   └─ NotYetReconciled
         │
         ▼
      ensure_jobs(spec, cr_meta, ctx)        # feature 007
         │
         ├─ Spawned { n }
         │     │
         │     ▼
         │   list_owned_jobs(api, cr_name)   # feature 008 (new)
         │     │
         │     ▼
         │   aggregate_job_outcomes(jobs)    # feature 008 (new, pure)
         │     │
         │     ├─ AllSucceeded ──▶ status_with_aggregated_outcome → Ready=True/ScanCompleted
         │     ├─ AnyFailed     ──▶ status_with_aggregated_outcome → Ready=False/ScanFailed
         │     └─ StillRunning  ──▶ status_with_orchestration_result → Ready=False/Scanning (feat 007)
         │
         ├─ NoImagesInScope ─────▶ status_with_orchestration_result (feat 007)
         ├─ BuildFailed ─────────▶ status_with_orchestration_result (feat 007)
         └─ RbacInsufficient ────▶ status_with_orchestration_result (feat 007)
```

FR-009 is enforced by the routing: aggregation only runs on the `Spawned`
branch.

## State transitions visible to the admin

```
  ┌──────────────────┐
  │  no status yet   │
  └────────┬─────────┘
           │ first reconcile
           ▼
  ┌──────────────────┐    invalid       ┌──────────────────┐
  │ NotYetReconciled │ ──────────────▶ │   InvalidSpec    │  ◀── terminal
  └────────┬─────────┘                  └──────────────────┘
           │ ensure_jobs Spawned
           ▼
  ┌──────────────────┐
  │     Scanning     │  ◀──── feat 007 (per-reconcile if Jobs still progressing)
  └────────┬─────────┘
           │ aggregate_job_outcomes
           │
           ├─ AllSucceeded ────▶ ScanCompleted (Ready=True) ──┐
           │                                                  │
           └─ AnyFailed ───────▶ ScanFailed (Ready=False) ────┤
                                                              │
                                                              │ admin fixes the issue;
                                                              │ next reconcile re-spawns
                                                              │ a Job for the new image
                                                              │ (per feat 007 dedup);
                                                              │ status reverts to Scanning
                                                              ▼
                                                           Scanning → eventually
                                                           ScanCompleted/ScanFailed
```

`NoImagesInScope`, `BuildFailed`, `RBACInsufficient` (feat 007) are unaffected
by this feature — they short-circuit aggregation per FR-009.

## FR → test mapping

| FR | Test |
|---|---|
| FR-001 | E2E: install operator, apply CR, wait for ensure_jobs, patch Job status, assert CR status updates within 5s (watch-driven). |
| FR-002 | Unit: `list_owned_jobs(api, cr_name)` against a labeled set returns the right Jobs; integration covered by E2E. |
| FR-003 | Unit: `aggregate_job_outcomes` with all-succeeded fixture → `AllSucceeded`; `status_with_aggregated_outcome` maps → `Ready=True/ScanCompleted`. E2E: kind cluster, real watch. |
| FR-004 | Unit: `aggregate_job_outcomes` with one-failed fixture → `AnyFailed { image_ref }`; mapper produces message naming the failing image. E2E: kind cluster, real watch. |
| FR-005 | Unit: all 4 cells of the decision table parameterized. |
| FR-006 | Unit: `aggregate_job_outcomes` populates `scanned` with one `ScannedImage` per succeeded Job; fields are non-empty. |
| FR-007 | Unit: `extract_image_ref_from_job` reads `init-pull` env var; missing env var → `None`. |
| FR-008 | Unit: `derive_sbom_location` over all 3 backend types + path-prefix variants. |
| FR-009 | Unit: routing in `reconcile()` (verified by inspecting reconcile flow control). |
| FR-010 | E2E: kill operator pod mid-run, verify status survives. |
| FR-011 | Unit: `job_to_cr_request(job_without_label)` returns `None`. |
| FR-012/FR-013 | Inherited: existing feat 002 + feat 007 tests run unchanged. |
| FR-014 | Static: grep — no `api.delete(job_name)` calls in feat 008 code. |
| FR-015 | Unit: `merge_scanned_images_append_only` — entries never removed; `image_ref` collisions favor newly-completed. |
