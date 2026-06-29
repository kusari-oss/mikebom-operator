# Phase 1 Data Model: Reconciler spawns scan Jobs

Records the Rust types added/modified by feature 007 and the FR → test mapping.
No CRD shape changes; all types here are internal to the `operator` crate.

## Modified types

### `crate::reconcile::namespace_scan::Ctx`

```rust
#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
    pub operator_namespace: String,   // NEW — populated from POD_NAMESPACE in main.rs
}
```

Migration: `main.rs` constructs the new field from the same `env::var("POD_NAMESPACE")` it already reads for the leader-election config (so no new env-var convention).

## New types

### `crate::reconcile::scan_orchestrator::OrchestrationResult`

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrchestrationResult {
    /// At least one Job exists for this CR (either created this cycle or
    /// already present). `distinct_images` is the total image count the
    /// orchestration ensured is covered (i.e., the size of the in-scope set).
    Spawned { distinct_images: usize },

    /// Target resolved to zero in-scope pods. Distinct from `Spawned { 0 }`:
    /// the latter shouldn't occur (would be an internal invariant violation).
    NoImagesInScope,

    /// `scan_job::build_scan_job` rejected the spec for a specific image.
    /// Surfaces as `BuildFailed` reason. The first failing image short-circuits
    /// orchestration; later images are not built.
    BuildFailed { image_ref: String, error: String },

    /// Kube API returned 403 on pod list or job create. Surfaces as
    /// `RBACInsufficient` reason. The first 403 short-circuits orchestration
    /// (research.md §6).
    RbacInsufficient {
        verb_resource: String,        // e.g., "list pods" or "create batch/v1.jobs"
        namespace: Option<String>,    // namespace the failed call targeted
        message: String,              // kube ErrorResponse.message verbatim
    },
}
```

Not `#[non_exhaustive]` — internal to the `operator` crate.

### `crate::reconcile::scan_orchestrator::OrchestrationError`

```rust
#[derive(Debug, thiserror::Error)]
pub enum OrchestrationError {
    /// Unexpected kube API error (500s, network failures). 403s and 409s are
    /// handled inline and never bubble up here.
    #[error("kube API error: {0}")]
    Kube(#[from] kube::Error),
}
```

Bounded: 403 and 409 are absorbed into `OrchestrationResult` variants so the
reconciler's status patch path has a single switch statement.

### `crate::reconcile::scan_orchestrator::CrMetaSnapshot`

```rust
pub struct CrMetaSnapshot {
    pub name: String,
    pub uid: String,
    pub namespace: String,
}
```

Minimal subset of `ObjectMeta` the orchestrator needs. Lets unit tests
synthesize a snapshot without a full `NamespaceScan` fixture.

### Status reason constants (in `crate::status`)

```rust
pub const NOT_YET_RECONCILED: &str = "NotYetReconciled";   // existing
pub const INVALID_SPEC: &str = "InvalidSpec";              // existing
pub const SCANNING: &str = "Scanning";                     // NEW
pub const NO_IMAGES_IN_SCOPE: &str = "NoImagesInScope";    // NEW
pub const RBAC_INSUFFICIENT: &str = "RBACInsufficient";    // NEW
pub const BUILD_FAILED: &str = "BuildFailed";              // NEW
```

The strings are stable identifiers that downstream tooling (admins reading
status, future feature 008's controller, the kind E2E) keys on. Treat them
like `pub` API.

### `crate::status::status_with_orchestration_result`

```rust
pub fn status_with_orchestration_result(
    base: NamespaceScanStatus,
    result: &OrchestrationResult,
    now: DateTime<Utc>,
) -> NamespaceScanStatus
```

Pure function. Implements the decision table from research.md §8. Reads only
the base status's `Ready` condition reason; overwrites status/reason/message
on the same condition. Leaves `last_reconciled_at` untouched (set by
`desired_status` upstream).

## State transitions

For a single CR over the course of a v0.7 deployment:

```
   ┌──────────────────┐
   │  no status yet   │
   └────────┬─────────┘
            │ first reconcile (feature 002)
            ▼
   ┌──────────────────┐    spec.target invalid     ┌──────────────────┐
   │ NotYetReconciled │ ───────────────────────▶  │   InvalidSpec    │  ◀── terminal until user edits CR
   └────────┬─────────┘                            └──────────────────┘
            │ feature 007 ensure_jobs(...)
            │
            ├─── Spawned { n: 1.. } ────────▶ Scanning
            ├─── NoImagesInScope  ───────────▶ NoImagesInScope
            ├─── BuildFailed     ────────────▶ BuildFailed
            └─── RbacInsufficient ───────────▶ RBACInsufficient
```

All four feature-007 reasons are still `Ready=False` — feature 008's
`ScanCompleted` with `Ready=True` is the only transition that flips to True.

## FR → test mapping

| FR | Test |
|---|---|
| FR-001 | unit: `collect_images_from_pods` over fixtures (3-container pod, init+main+ephemeral, phase Running/Pending/Succeeded/Failed/Unknown, empty/None image strings). |
| FR-002 | integration: kind cluster fixture with 2 distinct images → 2 Jobs created. |
| FR-003 | integration: `job.metadata.namespace == ctx.operator_namespace`. |
| FR-004 | unit: orchestrator-computed name matches `scan_job::job_name(cr_name, &short_image_hash(image_ref))` for the same inputs. |
| FR-005 | integration: spawned Job's `metadata.owner_references[0]` has `controller=true`, `block_owner_deletion=true`, `kind="NamespaceScan"`, `uid=cr.uid`. |
| FR-006 | unit: `status_with_orchestration_result(base, &Spawned { distinct_images: 3 }, now)` → reason `Scanning`. |
| FR-007 | unit: `status_with_orchestration_result(base, &NoImagesInScope, now)` → reason `NoImagesInScope`. |
| FR-008 | unit: `status_with_orchestration_result(base, &RbacInsufficient { .. }, now)` → reason `RBACInsufficient`; message includes verb+resource+namespace. |
| FR-009 | integration: invoke `ensure_jobs` twice with unchanged inputs; second call sees existing Jobs via 409 path; total Job count unchanged. |
| FR-010 | unit: `try_create_job_idempotent` with a fake `Api<Job>` returning 409 → `Ok(false)` (preexisting). |
| FR-011 | integration: wall-clock <30s for a 25-image fixture (loose bound; catches order-of-magnitude regressions). |
| FR-012 | inherited: existing feature 002 lib + E2E tests run unchanged in CI. |
| FR-013 | inherited: existing feature 003–006 `scan_job` lib tests run unchanged in CI. |
| FR-014 | static: no `Job.status` reads anywhere in the new code (verified by grep). |
| FR-015 | static: no `spec.schedule` reads anywhere in the new code (verified by grep). |

## Wire-level shape of a spawned Job

(For reference — produced by `build_scan_job(...)` + ownerReference injection.)

```yaml
apiVersion: batch/v1
kind: Job
metadata:
  name: nsscan-scan-prod-a1b2c3d                 # job_name(cr_name, short_hash)
  namespace: kusari-operator                      # Ctx::operator_namespace
  labels:
    app.kubernetes.io/name: mikebom-operator
    app.kubernetes.io/component: scan-job
    kusari.dev/namespace-scan: scan-prod
    kusari.dev/image-ref-hash: a1b2c3d
  ownerReferences:                                # NEW in feature 007
    - apiVersion: kusari.dev/v1alpha1
      kind: NamespaceScan
      name: scan-prod
      uid: <cr.uid>
      controller: true
      blockOwnerDeletion: true
spec:
  # ... feature 003's full Job spec (init-pull, mikebom-scan, output-upload)
```
