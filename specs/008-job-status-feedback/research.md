# Phase 0 Research: Status feedback from Job watch

This document records the decisions feature 008 makes before code lands. Each
decision is short: what we're doing, why, and what we considered. The plan
references each by its number (`research.md §N`).

## 1. Watch wiring

- **Decision**: Extend the existing `Controller::new(api, watcher::Config::default())` chain with `.watches(jobs_api, watcher::Config::default(), job_to_cr_request)`. The `jobs_api` is `Api::<Job>::namespaced(client, &operator_namespace)`, scoping watch traffic to the operator's namespace at the source.
- **Rationale**: kube-rs 0.97's `Controller::watches` is the canonical way to make a secondary resource enqueue the primary resource's reconcile. Namespace-scoping `jobs_api` is cheaper than cluster-wide watching because all spawned Jobs live in the operator's namespace (FR-003 from feature 007).
- **Alternatives**: (a) Label-selector scoping inside `watcher::Config` — defer; namespace-scoping is enough for v0.8. (b) `Controller::owns(Api<Job>, ...)` — kube-rs 0.97 doesn't expose `owns` separately; `watches` + a custom mapper is the idiom.

## 2. Mapping fn shape

- **Decision**: `move |job: Arc<Job>| -> Option<ObjectRef<NamespaceScan>>`, capturing `operator_namespace` by clone. Returns `None` for Jobs without the `kusari.dev/namespace-scan` label (FR-011 — ignore unowned events). Returns `Some(ObjectRef::new(cr_name).within(operator_namespace))` otherwise. The actual signature kube-rs expects is `Fn(Arc<Job>) -> impl IntoIterator<Item=ObjectRef<NamespaceScan>>`; `Option<T>` satisfies the `IntoIterator` bound.
- **Rationale**: The label is the source of truth for CR identification — it's a single hashmap lookup per event, much cheaper than iterating `ownerReferences`. Feature 007's builder already sets this label on every Job; feature 008 doesn't add a new one.
- **Alternatives**: Walk `ownerReferences` and find the `NamespaceScan` ref — works but O(N) per event with N typically small; label lookup is O(1) and simpler.

## 3. Operator namespace in mapper

- **Decision**: The mapping closure captures `operator_namespace: String` via `move`. Constructed at `main.rs` Controller setup time from the same `POD_NAMESPACE` env var feature 007 already reads.
- **Rationale**: `ObjectRef::within(...)` needs the CR's namespace string; the mapper has no access to `Ctx`. The capture-by-clone keeps the mapper a pure function of its input.
- **Alternatives**: Read from `kube::Client::default_namespace()` — fragile (depends on kubeconfig); rejected.

## 4. `imageRef` source

- **Decision**: Read `IMAGE_REF` from the Job's `init-pull` container's env var. Feature 003's builder already sets this. The extractor walks `job.spec.template.spec.init_containers[]`, finds the container named `"init-pull"`, then finds the env var named `"IMAGE_REF"` and returns its `value`. Missing init-pull container or missing env var → return `None` (that Job contributes nothing to `scanned` but still counts for aggregation).
- **Rationale**: No new label on the Job means no builder change. Image refs contain `:` and `/` which would need DNS-1123 sanitization to fit in a label value; env vars have no such restriction. The IMAGE_REF env var is already present, so we just read it.
- **Alternatives**: New `kusari.dev/image-ref` label — rejected; needs sanitization, requires a feature 003 builder change, and creates two sources of truth.

## 5. `sbomLocation` derivation

- **Decision**: A pure helper `derive_sbom_location(spec: &NamespaceScanSpec, short_hash: &str) -> String` produces one of three URL schemes:
  - `pvc://<claimName>/<pathPrefix>/<short_hash>.json` (PVC)
  - `s3://<bucket>/<pathPrefix>/<short_hash>.json` (S3)
  - `oci://<registry>/<repository>:<short_hash>` (OCI)

  The function reads `spec.output.backend_type` + the matching `spec.output.{pvc,s3,oci}` block. `short_hash` comes from the Job's `kusari.dev/image-ref-hash` label. The `<ext>` is always `.json` for v0.7's three scan formats.
- **Rationale**: Single function, three arms — straightforward. The `.json` extension is universal across the v0.7 scan formats. `pathPrefix` is appended only when set + non-empty (matches feature 004/005's PVC/S3 conventions).
- **Caveat**: When the user edits the CR's `output` field mid-scan, this function returns a URL based on the *current* CR spec — which disagrees with where the *spawned Job* actually wrote. This is an accepted limitation for v0.8 (the edge case requires admin action mid-scan); documented in spec.md Edge Cases. A future feature can read the actual backend from the output-upload container's env to be precise.
- **Alternatives**: Read backend from the Job's output-upload container env vars — more accurate but more code; deferred.

## 6. Aggregation decision table

- **Decision**: `aggregate_job_outcomes(jobs: &[Job], _spec: &NamespaceScanSpec) -> AggregatedOutcome` implements:

  | Predicate | Outcome |
  |---|---|
  | `jobs.is_empty()` | `StillRunning` |
  | `jobs.iter().any(is_job_finally_failed)` | `AnyFailed { image_ref: <first failing> }` |
  | `jobs.iter().all(is_job_succeeded)` | `AllSucceeded { scanned: <one ScannedImage per Job> }` |
  | otherwise | `StillRunning` |

  Helpers:
  - `is_job_succeeded(job) -> bool`: `job.status.as_ref().and_then(|s| s.succeeded).unwrap_or(0) >= 1`
  - `is_job_finally_failed(job) -> bool`: `job.status.as_ref().and_then(|s| s.failed).unwrap_or(0) >= backoff_limit(job) + 1`
  - `backoff_limit(job) -> i32`: `job.spec.as_ref().and_then(|s| s.backoff_limit).unwrap_or(6)` (k8s default)
- **Rationale**: Encodes the FR-005 decision table literally. The "empty list" row prevents ScanCompleted-flapping in the narrow window where TTL fires between create and list.
- **Alternatives**: Use `job.status.conditions[type=Complete/Failed]` — works but requires JobCondition parsing; the `succeeded`/`failed` counters are simpler and equally reliable.

## 7. Append-only merge of `scannedImages[]`

- **Decision**:

  ```rust
  fn merge_scanned_images_append_only(
      existing: &[ScannedImage],
      newly_completed: Vec<ScannedImage>,
  ) -> Vec<ScannedImage> {
      let mut by_ref: BTreeMap<String, ScannedImage> = existing
          .iter()
          .cloned()
          .map(|s| (s.image_ref.clone(), s))
          .collect();
      for s in newly_completed {
          by_ref.insert(s.image_ref.clone(), s);  // newest wins
      }
      by_ref.into_values().collect()
  }
  ```

  `BTreeMap` gives deterministic ordering by `image_ref` for stable test assertions. Newest-wins on duplicate `image_ref` (per the same image being re-scanned in a future feature). No entry is ever removed by this function — pruning is deferred (FR-015).
- **Rationale**: Single function, fully testable. The "newest wins" semantics handle the future re-scan case without requiring a separate code path.
- **Alternatives**: Vector + linear search — O(N²) but N ≤ 25; rejected for clarity, not perf.

## 8. Condition reason + status constants

- **Decision**: Add to `crate::status`:

  ```rust
  pub const STATUS_TRUE: &str = "True";
  pub const REASON_SCAN_COMPLETED: &str = "ScanCompleted";
  pub const REASON_SCAN_FAILED: &str = "ScanFailed";
  ```

  Plus the mapper `status_with_aggregated_outcome(base, existing, outcome, now)` per data-model.md.
- **Rationale**: Matches feature 007's pattern (`REASON_SCANNING`, etc.). The status-mapper function lives in `status.rs` next to `status_with_orchestration_result` so all condition logic is in one file.
- **Alternatives**: Inline the mapping in `reconcile()` — rejected; mixing I/O with the decision table makes unit testing impossible.

## 9. Kind E2E scaffolding

- **Decision**: New `e2e/tests/job_status_feedback.rs` reuses the chart-install + image-build + helm-wait pattern from `reconciler_skeleton.rs`. Three test scenarios:
  - **t-success**: Apply CR + pod with image `X`, wait for Job to be spawned, patch Job status to `succeeded=1`, assert CR transitions to `Ready=True/ScanCompleted` within 5s.
  - **t-failure**: Apply CR + pod with image `X`, wait for Job, patch Job status to `failed=backoffLimit+1`, assert `Ready=False/ScanFailed` within 5s with message naming `X`.
  - **t-mixed**: Apply CR + 2 pods with images `X,Y`, wait for 2 Jobs, patch one to succeeded and the other to failed, assert `ScanFailed` (failure dominates).

  Shared chart-install scaffolding factored into `e2e/tests/common/mod.rs` (small ~80-line module) to avoid duplicating across `reconciler_skeleton.rs` and the new file. Existing `reconciler_skeleton.rs` is not touched in this feature; the common module is added as a NEW file and only consumed by the new test.
- **Rationale**: Watch behavior is runtime-only; the in-process E2E approach from feature 007 can't exercise `Controller::watches`. Patching Job status directly (rather than waiting for natural completion via the mikebom-scan container) keeps the tests fast (~10s per case) and deterministic.
- **Alternatives**: Spawn real Jobs and wait for natural completion — flaky (mikebom-scan image pulls, network) and slow (~minutes per case); rejected. Skip the failure case — rejected; US2 is P1.
