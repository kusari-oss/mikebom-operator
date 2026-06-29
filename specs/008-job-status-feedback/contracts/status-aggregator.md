# Contract: `status_aggregator` + `status_with_aggregated_outcome`

Internal contract for the two new functions feature 008 introduces. Not part
of the `operator` crate's public surface, but the boundary is worth pinning
so feature 009 (schedule honoring) can extend it without rewriting.

## `aggregate_job_outcomes`

### Signature

```rust
pub fn aggregate_job_outcomes(
    jobs: &[Job],
    spec: &NamespaceScanSpec,
) -> AggregatedOutcome;
```

### Inputs

- **`jobs`**: every `batch/v1.Job` whose `kusari.dev/namespace-scan` label
  matches the CR's name. Caller (`list_owned_jobs`) is responsible for the
  label filter.
- **`spec`**: the CR's `NamespaceScanSpec`. Used to derive `sbom_location`
  for the `AllSucceeded` arm. Read-only borrow.

### Outputs

Per the decision table in research.md §6:

- **`AggregatedOutcome::StillRunning`**: `jobs.is_empty()` OR no Job is finally
  failed AND not all Jobs have succeeded.
- **`AggregatedOutcome::AnyFailed { image_ref }`**: at least one Job has
  `.status.failed >= .spec.backoffLimit + 1`. `image_ref` is the
  init-pull `IMAGE_REF` env var of the first failing Job in iteration order
  (label-sort order is deterministic enough — `image_ref_hash` is a 7-char
  SHA prefix, sorts identically to the underlying image strings in practice).
- **`AggregatedOutcome::AllSucceeded { scanned }`**: every Job has
  `.status.succeeded >= 1`. `scanned.len() == jobs.len()` (one
  `ScannedImage` per Job).

### Invariants

1. **Pure**. No I/O, no mutation, no panics. Same inputs always produce the
   same output.
2. **Failure dominates** (FR-005). If both succeeded and failed Jobs exist,
   the outcome is `AnyFailed`.
3. **Empty defaults to `StillRunning`**. Avoids `ScanCompleted` flapping
   when the TTL-vs-list race resolves with zero Jobs visible.
4. **No partial `AllSucceeded`**. Every entry in `scanned` MUST have non-empty
   `image_ref`, non-empty `sbom_location`, and an RFC 3339 `completed_at`.
   If `extract_image_ref_from_job` returns `None` for any succeeded Job, that
   Job is excluded from `scanned` BUT the outcome is still `AllSucceeded` if
   every Job succeeded. (Practically, missing IMAGE_REF means a malformed
   Job — not our problem to error on; just record what we can.)
5. **No mutation of `jobs`**. The slice is read-only.

## `status_with_aggregated_outcome`

### Signature

```rust
pub fn status_with_aggregated_outcome(
    base: NamespaceScanStatus,
    existing: Option<&NamespaceScanStatus>,
    outcome: &AggregatedOutcome,
    now: DateTime<Utc>,
) -> NamespaceScanStatus;
```

### Inputs

- **`base`**: the status produced by feature 007's
  `status_with_orchestration_result` (always `Ready=False/Scanning` for the
  `Spawned` orchestration arm — feature 008 is only called on that arm).
- **`existing`**: the prior patched status (if any). Used to preserve
  `lastTransitionTime` when the (status, reason) pair is unchanged.
- **`outcome`**: from `aggregate_job_outcomes`.
- **`now`**: the reconcile start timestamp. Used for `lastTransitionTime`
  on transitions.

### Outputs

Per the decision table in research.md §8 + data-model.md:

- `AllSucceeded { scanned }` → `Ready=True/ScanCompleted`; merge `scanned`
  into `status.scannedImages[]` (append-only per FR-015); advance
  `status.lastScanCompletedAt` to `max(scanned.completedAt)`.
- `AnyFailed { image_ref }` → `Ready=False/ScanFailed`;
  `status.scannedImages[]` passes through unchanged.
- `StillRunning` → return `base` unchanged.

### Invariants

1. **Pure**. No I/O, no mutation.
2. **Append-only `scannedImages[]`** (FR-015). Entries are never removed.
   Duplicate `image_ref` keys are resolved by newest-wins (`completed_at`
   from `outcome.scanned` overrides any prior entry).
3. **`lastTransitionTime` preserved on idempotent re-calls** (matches feature
   002 + feature 007 conventions). When `(existing.conditions[Ready].status,
   .reason)` matches the new condition's `(status, reason)`, preserve the
   prior timestamp; otherwise advance to `now`.
4. **`lastScanCompletedAt` only advances forward**. Even if `scanned`
   contains an `older` completedAt, `lastScanCompletedAt` is set to the
   max of (existing value, all `scanned.completedAt`).

## Helper contracts

### `is_job_succeeded(job: &Job) -> bool`

`job.status.as_ref().and_then(|s| s.succeeded).unwrap_or(0) >= 1`. No
ambiguity — k8s guarantees `succeeded >= 1` only on successful completion.

### `is_job_finally_failed(job: &Job) -> bool`

`failed_count >= backoff_limit + 1`. The `backoff_limit` comes from the
Job's spec (`job.spec.backoff_limit.unwrap_or(6)` — the k8s default). The
`+ 1` mirrors k8s's documented semantics: a Job is "finally failed" when
the `(backoffLimit + 1)`th pod fails.

### `extract_image_ref_from_job(job: &Job) -> Option<String>`

Walks `job.spec.template.spec.init_containers[]`, finds the container named
`"init-pull"`, then finds the env var named `"IMAGE_REF"`, returns its
`value` (with `value_from` always being `None` here per feature 003's
builder). Missing container or env var → `None`.

### `derive_sbom_location(spec: &NamespaceScanSpec, short_hash: &str) -> String`

Switches on `spec.output.backend_type`:

- `Pvc`: `pvc://<claim>/<prefix>/<short_hash>.json`, omitting `<prefix>/`
  when `pvc.path_prefix` is unset or empty.
- `S3`: `s3://<bucket>/<prefix>/<short_hash>.json`, same prefix rule.
- `Oci`: `oci://<registry>/<repository>:<short_hash>` (no extension; OCI
  artifacts are tag-addressed).

The pure helper assumes the relevant `spec.output.{pvc,s3,oci}` block is
present (orchestrator's `BuildFailed` arm would have caught the missing
block). For defense-in-depth, if the matching block is `None`, returns a
fallback string `"<backend>://unknown"` so the status update doesn't
panic; this is a degenerate case that should never happen in practice.

### `list_owned_jobs(api: &Api<Job>, cr_name: &str) -> Result<Vec<Job>, kube::Error>`

`api.list(&ListParams::default().labels(&format!("kusari.dev/namespace-scan={cr_name}")))`,
returning the items vec. The only kube error path is propagated to the
reconciler, where the existing `error_policy` requeues with backoff.

### `merge_scanned_images_append_only(existing: &[ScannedImage], newly_completed: Vec<ScannedImage>) -> Vec<ScannedImage>`

Per research.md §7: BTreeMap-by-image_ref, newest wins on duplicates.
Always sorted by `image_ref` in the output. No entry is ever removed.

## Versioning

The `AggregatedOutcome` enum is internal (not part of any crate's public
surface). The user-visible vocabulary additions (`ScanCompleted`,
`ScanFailed` status reasons) ARE part of the v1alpha1 `Ready` condition
contract — additive per constitution IV. Renaming either is a breaking
change.

## Non-goals (out of scope for feature 008)

- **Deleting completed Jobs** (FR-014). The aggregator does not call
  `api.delete()`; cleanup is governed by `ttlSecondsAfterFinished`.
- **Re-scanning on completion**. The orchestrator's idempotency means a
  completed Job is observed (not respawned) on every reconcile.
  Re-scanning lands with schedule honoring.
- **Capturing `resolvedSha`**. Stays `None` in v0.8 (deferred per
  Assumptions).
- **`sbomLocation` reflecting mid-scan output edits**. The function reads
  the current CR spec; if the admin edited `spec.output` after the Job
  was spawned, the URL may not match where the SBOM actually landed.
  Documented as a known limitation; deferred to a follow-up.
- **Multi-CR Job sharing**. Feature 007 already enforces per-CR ownership;
  aggregation honors the same boundary (one CR's `scannedImages[]` is
  populated only from Jobs labeled with that CR's name).
