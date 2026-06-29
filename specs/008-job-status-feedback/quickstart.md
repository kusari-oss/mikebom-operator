# Quickstart: Status feedback from Job watch

Two perspectives: cluster admin (using v0.8) and contributor (extending the
aggregator).

## Cluster admin: upgrading from v0.7 to v0.8

**Chart-side**: no breaking changes. No CRD changes, no RBAC changes — the
existing ClusterRole already covers everything feature 008 needs (`jobs:get,
list,watch`). A standard upgrade works:

```sh
helm upgrade mikebom-operator charts/mikebom-operator \
  -n kusari-operator --wait --timeout 60s
```

### What changes for the user

**Before v0.8**: applying a `NamespaceScan` CR transitioned status to
`Ready=False / reason=Scanning` (feature 007) — and stayed there forever,
even when the underlying scan Jobs had been complete for hours. The only way
to know if scanning had finished was `kubectl get jobs -n kusari-operator -l
kusari.dev/namespace-scan=<cr-name>`.

**After v0.8**: as Jobs transition, the CR's status follows in real time
(watch-driven, sub-second after the API server propagates the event):

```yaml
status:
  conditions:
    - type: Ready
      status: "True"                            # was "False" before v0.8
      reason: ScanCompleted                     # NEW in v0.8
      message: "scanned 5 distinct images successfully"
      lastTransitionTime: "2026-06-28T14:23:01Z"
  lastReconciledAt: "2026-06-28T14:23:01Z"
  lastScanCompletedAt: "2026-06-28T14:22:58Z"   # NEW: populated for the first time
  scannedImages:                                # NEW: populated for the first time
    - imageRef: "nginx:1.27.0"
      sbomLocation: "s3://sboms-prod/team-a/a1b2c3d.json"
      completedAt: "2026-06-28T14:22:51Z"
    - imageRef: "redis:7.4.0"
      sbomLocation: "s3://sboms-prod/team-a/e4f5a6b.json"
      completedAt: "2026-06-28T14:22:58Z"
    # ... three more
```

### `kubectl wait` finally works

The single biggest UX win: standard Kubernetes ergonomics now apply to the
operator. CI/CD pipelines can block on scan completion without polling:

```sh
kubectl apply -f my-namespacescan.yaml
kubectl wait --for=condition=Ready namespacescan/scan-prod \
  -n kusari-operator --timeout=10m
# exits 0 when Ready=True (= ScanCompleted), non-zero on timeout
```

For failure handling:

```sh
# Check the most recent scan outcome
kubectl get namespacescan scan-prod -n kusari-operator \
  -o jsonpath='{.status.conditions[?(@.type=="Ready")].reason}'
# Prints "ScanCompleted", "ScanFailed", "Scanning", "NoImagesInScope", etc.
```

### New status reasons to know about

Two added in v0.8:

| Reason | `status` | Meaning | Remediation |
|---|---|---|---|
| `ScanCompleted` | `True`  | All owned Jobs succeeded. SBOMs are in the configured output backend. | Use the SBOMs! Enumerate via `status.scannedImages[].sbomLocation`. |
| `ScanFailed`    | `False` | At least one Job exhausted its retry budget (typically `backoffLimit + 1 = 7` failed pods by default). | Inspect the failing Job's pods: `kubectl logs -n kusari-operator -l job-name=<failing-job>`. After fixing the underlying issue (image ref, RBAC, credentials), either patch the failing pod's image (triggers feature 007 to spawn a new Job for the new image) OR manually delete the failed Job (feature 007 will respawn on the next reconcile). |

### `scannedImages[]` is append-only

Once an image's scan completes, its entry stays in `status.scannedImages[]`
even if the underlying pod is deleted or its image is changed. This is
intentional — the array is a *manifest* of what was scanned, not a snapshot
of what's currently in scope. To purge stale entries: delete the CR and
re-apply (a future feature may add explicit pruning).

## Contributor: extending the aggregator

The aggregator lives at
`crates/operator/src/reconcile/status_aggregator.rs`. Its public surface is
two functions:

```rust
pub fn aggregate_job_outcomes(jobs: &[Job], spec: &NamespaceScanSpec)
    -> AggregatedOutcome;

pub async fn list_owned_jobs(api: &Api<Job>, cr_name: &str)
    -> Result<Vec<Job>, kube::Error>;
```

Plus the status mapper in `crates/operator/src/status.rs`:

```rust
pub fn status_with_aggregated_outcome(
    base: NamespaceScanStatus,
    existing: Option<&NamespaceScanStatus>,
    outcome: &AggregatedOutcome,
    now: DateTime<Utc>,
) -> NamespaceScanStatus;
```

See [contracts/status-aggregator.md](./contracts/status-aggregator.md) for
the full invariants.

### Adding a new aggregation outcome

E.g., feature 009 might want a `ScanStale` outcome to signal "completed
but the schedule says it's time to re-scan." Steps:

1. Add a new variant to `AggregatedOutcome`.
2. Add a new `REASON_*` constant in `crate::status`.
3. Add a row to `status_with_aggregated_outcome`'s decision table.
4. Add a row to the architecture/CRD-reference docs.
5. New unit tests for the variant + mapping.

### Adding a new SBOM backend

If feature N+ adds a fourth output backend (e.g., HTTP POST to a webhook):

1. Add a new arm to `derive_sbom_location` (`http://<endpoint>/<short_hash>`?).
2. Add a unit test for the new URL shape.
3. The aggregator's `AllSucceeded` arm picks up the new format
   automatically via the `derive_sbom_location` call.

### Capturing `resolvedSha`

Currently `None` (Assumptions). To populate it, you'd need to read either:

- the target pod's `status.containerStatuses[].imageID` (conflates
  "what's running" with "what was scanned"), OR
- the Job's init-pull container output (would require instrumenting
  `crane export` to surface the digest, or adding a separate `crane digest`
  step).

Both have v0.x-scope-creep risk. Recommended path: a follow-up feature that
adds a new step to the Job's pod template to write the digest to a file in
`/workdir/`, then the output-upload container reads it as a label or annotation
on the Job's pod's status. Pure data flow; no SBOM-content parsing.

## Running the gated E2E locally

The existing feature 007 in-process E2E continues to work as-is:

```sh
kind create cluster --config e2e/kind-cluster.yaml
MIKEBOM_OPERATOR_E2E=1 cargo test --test reconciler_spawns_job
```

The new feature 008 E2E requires a *real operator pod* in kind:

```sh
# Prerequisites (same as feature 002's reconciler_skeleton.rs):
kind create cluster --config e2e/kind-cluster.yaml
docker build -t mikebom-operator:dev .
kind load docker-image mikebom-operator:dev --name mikebom-operator-e2e

# Run:
MIKEBOM_OPERATOR_E2E=1 cargo test --test job_status_feedback
```

Three test scenarios:

- **t-success** (~10s): apply CR + one pod with image `X`; wait for ensure_jobs to spawn the Job; patch Job status to `succeeded=1`; assert CR transitions to `Ready=True/ScanCompleted` within 5s.
- **t-failure** (~10s): same setup; patch Job status to `failed=backoffLimit+1`; assert `Ready=False/ScanFailed` within 5s.
- **t-mixed** (~15s): apply CR + two pods (images `X` and `Y`); patch one Job's status to succeeded, the other to failed; assert `ScanFailed` (failure dominates).
