# Quickstart: Reconciler spawns scan Jobs

Two perspectives: cluster admin (using v0.7) and contributor (extending the
orchestrator).

## Cluster admin: upgrading from v0.6 to v0.7

**Chart-side**: no breaking changes. The Helm chart's RBAC already grants the
permissions feature 007 needs (verified by inspection — see plan.md
Constitution Check row VII). A standard upgrade works:

```sh
helm upgrade mikebom-operator charts/mikebom-operator \
  -n kusari-operator --wait --timeout 60s
```

**CRD-side**: no schema changes. Existing `NamespaceScan` CRs continue to
work without modification.

### What changes for the user

Before v0.7: applying a valid `NamespaceScan` CR resulted in
`status.conditions[Ready] = { status: False, reason: NotYetReconciled }`
indefinitely. The operator never created scan Jobs.

After v0.7: applying the same CR causes the operator to enumerate pods in the
target namespaces and create one `batch/v1.Job` per distinct in-scope image
(filtered to phase `Running` or `Pending`). The CR's status transitions:

```yaml
status:
  conditions:
    - type: Ready
      status: "False"
      reason: Scanning                          # NEW in v0.7
      message: "scanning 5 distinct images across target namespaces"
      lastTransitionTime: "2026-06-28T14:23:01Z"
  lastReconciledAt: "2026-06-28T14:23:01Z"
```

### Observing the spawned Jobs

```sh
kubectl get jobs -n kusari-operator -l kusari.dev/namespace-scan=scan-prod
# NAME                            COMPLETIONS   AGE
# nsscan-scan-prod-a1b2c3d        0/1           12s
# nsscan-scan-prod-e4f5a6b        0/1           12s
# ...
```

The `kusari.dev/namespace-scan=<cr-name>` label selector returns all Jobs
owned by a single CR. Each Job's `metadata.ownerReferences` points at the CR
with `controller=true`, so deleting the CR cascades:

```sh
kubectl delete namespacescan scan-prod -n kusari-operator
# All five Jobs and their pods are garbage-collected by Kubernetes within ~30s.
```

### New status reasons to know about

| Reason | What it means | Remediation |
|---|---|---|
| `NotYetReconciled` | Pre-v0.7 placeholder. Will not appear for valid specs after upgrade. | — |
| `InvalidSpec` | `spec.target` has neither `namespaces` nor `labelSelector`. | Edit the CR. |
| `Scanning` | One or more scan Jobs exist for this CR (v0.7+). | Wait for completion (feature 008 will surface). |
| `NoImagesInScope` | Target resolved to zero `Running`/`Pending` pods. | Apply workloads to the target namespace, or wait. |
| `BuildFailed` | An image's Job manifest couldn't be built (e.g., `output.type=Pvc` without `pvc.claimName`). | Fix the `output` block on the CR. The message names the failing image and the builder error. |
| `RBACInsufficient` | The operator lacks `pods:list` in a target namespace, or `jobs:create` in its own namespace. | Re-install the chart with `rbac.create=true` (the default), or grant the missing verb manually. |

## Contributor: extending the orchestrator

The orchestrator lives at
`crates/operator/src/reconcile/scan_orchestrator.rs`. Its public surface is a
single async function:

```rust
pub async fn ensure_jobs(
    spec: &NamespaceScanSpec,
    cr_meta: &CrMetaSnapshot,
    ctx: &Ctx,
) -> Result<OrchestrationResult, OrchestrationError>;
```

See [contracts/reconciler-orchestrator.md](./contracts/reconciler-orchestrator.md)
for the full contract and invariants.

### Adding a new status reason

1. Add the constant to `crate::status` (alongside `SCANNING`, `BUILD_FAILED`,
   etc.).
2. Add the corresponding `OrchestrationResult` variant.
3. Add the row to `status_with_orchestration_result`'s decision table.
4. Add a unit test in `status::tests` for the new mapping.
5. Add a row to `docs/crd-reference.md`'s "Condition reasons" table.

### Adding a new pod-source kind (e.g., honoring Deployments)

`target.kinds` is currently Pod-only (Assumptions section in spec). Widening
to Deployments means:

1. In `scan_orchestrator`, add a `list_deployments(...)` helper.
2. After listing Deployments, walk `deployment.spec.template.spec.{init_containers,containers,ephemeral_containers}[].image`
   into the same `BTreeSet<String>` the Pod path produces.
3. Update `collect_images_from_pods` (rename to `collect_images_from_workloads`?)
   to accept a unified workload-image enumeration trait.
4. Add chart RBAC for `apps/v1.deployments:list,watch` (already present —
   verified by `grep deployments charts/mikebom-operator/templates/rbac.yaml`).
5. New unit + integration tests.
6. Update the spec's Assumptions and `target.kinds` Edge Case bullet.

This widening is a separate feature, not part of 007.

### Adding Job-status feedback (feature 008 preview)

Feature 008 will:

1. Add a `Controller::watches(Job, ...)` mapping fn that enqueues the owning
   `NamespaceScan` when a Job transitions.
2. In `reconcile()`, after the orchestrator runs, list Jobs owned by the CR
   and inspect their `.status.{succeeded, failed, active}` fields.
3. Extend the status-reason decision table to produce `ScanCompleted`
   (with `Ready=True`) when all owned Jobs have `succeeded=1`, or
   `ScanFailed` when any has `failed=backoffLimit+1`.
4. Populate `status.scannedImages[]` from completed Jobs' labels +
   `status.completion_time`.

The feature 007 orchestrator contract is designed to leave room for this —
`ensure_jobs` deliberately returns a result that doesn't claim "scan
finished", only "scan dispatched."

## Running the kind dry-run E2E locally

The existing builder dry-run E2E still works as-is:

```sh
kind create cluster --config e2e/kind-cluster.yaml
MIKEBOM_OPERATOR_E2E=1 cargo test --test scan_job_dryrun
```

The new feature-007 E2E:

```sh
MIKEBOM_OPERATOR_E2E=1 cargo test --test reconciler_spawns_job
```

Runs in <10s per case (no chart install, no image build). It applies a CR +
fixture pods directly to the cluster via `kube::Api`, then invokes
`scan_orchestrator::ensure_jobs(...)` in-process. Asserts Jobs land in the
operator's namespace with the expected `ownerReferences`, and that a second
invocation produces zero new Jobs.
