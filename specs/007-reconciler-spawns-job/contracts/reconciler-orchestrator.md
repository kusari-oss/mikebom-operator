# Contract: `scan_orchestrator::ensure_jobs`

Internal contract for the function the reconciler calls per cycle. Not part of
the `operator` crate's public API; this contract exists so features 008+ can
extend the surface without rewriting it.

## Signature

```rust
pub async fn ensure_jobs(
    spec: &NamespaceScanSpec,
    cr_meta: &CrMetaSnapshot,
    ctx: &Ctx,
) -> Result<OrchestrationResult, OrchestrationError>;
```

## Inputs

- **`spec`**: read-only borrow of the CR's `spec` (`NamespaceScanSpec`). The
  function uses `spec.target.namespaces`, `spec.target.label_selector`,
  `spec.mikebom_image`, `spec.scan_format`, and `spec.output`. It does NOT read
  `spec.schedule` (FR-015) or `spec.target.kinds` other than as documented in
  the spec's Assumptions (Pod-only honoring in v0.7).

- **`cr_meta`**: `CrMetaSnapshot { name, uid, namespace }`. The CR's `name`
  and `uid` are baked into spawned Job `ownerReferences`. The `namespace`
  field is the CR's own namespace (which, in the v0.7 chart shape, equals
  `ctx.operator_namespace`, but the contract does not require it to).

- **`ctx`**: `Ctx { client, operator_namespace }`. The `operator_namespace`
  is the destination for created Jobs (FR-003); the `client` is used for both
  pod list (against target namespaces) and Job create (against
  `operator_namespace`).

## Outputs

- **`Ok(OrchestrationResult::Spawned { distinct_images })`** — at least one
  in-scope pod exists; all distinct images either had a Job created this
  cycle or already had one from a prior cycle (the 409 path). `distinct_images`
  is the size of the in-scope image set, not the count of newly-created Jobs.

- **`Ok(OrchestrationResult::NoImagesInScope)`** — target resolved to zero
  in-scope pods (after the phase filter). Includes the case where a target
  namespace doesn't exist (kube returns 404 on list — treated identically to
  "namespace exists but is empty").

- **`Ok(OrchestrationResult::BuildFailed { image_ref, error })`** — for at
  least one image, `scan_job::build_scan_job(...)` returned `Err`. The first
  failing image short-circuits orchestration; the remaining images are not
  attempted. `image_ref` is the literal image string; `error` is
  `format!("{}", BuildScanJobError)`.

- **`Ok(OrchestrationResult::RbacInsufficient { verb_resource, namespace, message })`**
  — kube returned 403 on a pod list or a Job create. First 403 short-circuits.
  The fields name the failing API call so the user-facing status message can
  cite the specific RBAC verb to grant.

- **`Err(OrchestrationError::Kube(_))`** — any kube error that is NOT 403, 409
  (handled inline), or 404-on-namespace (handled inline). Includes 500s,
  network failures, and watch desyncs. The reconciler's error_policy retries
  with backoff.

## Invariants

1. **No partial spawn on RBAC failure** (research.md §6, constitution III).
   If the function would return `RbacInsufficient`, it MUST NOT have created
   any Job in `ctx.operator_namespace`. The order of operations is therefore:
   list all pods first (failing fast on the first 403), then attempt creates.

2. **Idempotency** (FR-009). For fixed `(spec, cr_meta, observable cluster
   state)`, two sequential calls produce the same `OrchestrationResult` variant.
   The second call's `Spawned.distinct_images` equals the first's (the second
   call sees the first's Jobs via the 409 path and counts them in the result).

3. **Side-effect bound**. The function:
   - Reads pods in `spec.target.namespaces`.
   - Creates Jobs in `ctx.operator_namespace`.
   - Does NOT patch CR status (that's the reconciler's job).
   - Does NOT delete Jobs (Kubernetes garbage-collects via owner references on
     CR delete; v0.7 has no "delete orphan Jobs" path).
   - Does NOT watch Jobs (`Job.status` reads happen in feature 008).

4. **Determinism of Job name** (FR-004). For a given `(cr_meta.name, image_ref)`,
   the Job name is `scan_job::job_name(cr_meta.name, &scan_job::short_image_hash(image_ref))`.
   Both helpers are re-exported `pub(crate)` for this purpose.

5. **OwnerReference shape** (FR-005). Every Job created by the function has
   exactly one `OwnerReference` with:
   - `api_version = "kusari.dev/v1alpha1"`
   - `kind = "NamespaceScan"`
   - `name = cr_meta.name`
   - `uid = cr_meta.uid`
   - `controller = Some(true)`
   - `block_owner_deletion = Some(true)`

## Non-goals (out of scope for feature 007)

- **Reading `Job.status`** — `scan_job_completed` / `scan_job_failed` belong
  to feature 008.
- **Honoring `spec.schedule`** — `cron`/`interval` belong to a separate feature.
- **Pod watching for instant re-reconcile on workload change** — relies on the
  Controller's existing requeue cadence (5 minutes for valid specs per feature
  002).
- **Cleaning up Jobs whose images no longer appear in target pods** — feature
  007 spawns; future features sweep.
- **Multi-CR concurrency limits** — bounded only by kube-rs Controller's
  default reconcile concurrency.

## Versioning

This contract is internal (not part of any crate's public surface), but the
status-reason string constants (`Scanning`, `NoImagesInScope`,
`RBACInsufficient`, `BuildFailed`) ARE part of the user-visible CRD status
vocabulary. Per constitution IV they are additive to v1alpha1's `Ready`
condition; renaming any of them is a breaking change and requires the same
treatment as a CRD shape change.
