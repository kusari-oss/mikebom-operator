# Architecture

## USE pattern

The operator does **not** statically link `mikebom`. It orchestrates
ephemeral `batch/v1` Job pods that run the published
`ghcr.io/kusari-oss/mikebom:<tag>` image. This keeps the operator binary
small and decouples release cadence: mikebom continues releasing on
`v*-alpha.*` tag pushes; the operator and its Helm chart version
independently.

See the bootstrap plan (`sparkling-chasing-bee.md` §2 Decision) for the
full rationale and the alternative (a 4th crate in the mikebom repo) that
was rejected.

## Three-container Job choreography

Per plan §3, each scan Job is composed of three containers sharing an
`emptyDir`-backed `/workdir`:

1. **`init-pull`** — `skopeo copy docker://<image-ref> dir:/workdir/image`
   plus a layer-extract helper that flattens the OCI image to
   `/workdir/rootfs`.
2. **`mikebom-scan`** — runs
   `mikebom sbom scan --path /workdir/rootfs --format <format>
    --output <format>=/workdir/out/<sha>.<ext>`.
3. **`output-upload`** — small uploader image that pushes
   `/workdir/out/<sha>.*` to the configured backend (PVC, S3, or OCI).

If `mikebom sbom scan --image <ref>` lands upstream later, this collapses
to a single-container Job. The CRD shape is unaffected.

## Source of truth for CRDs

Constitution principle VII (Helm Chart Lockstep) makes the Rust source
the single source of truth for every CRD the operator owns:

```text
crates/operator/src/crds/namespace_scan.rs   (canonical: NamespaceScanSpec + derives)
                    │
                    │  kube::CustomResourceExt::crd()  +  serde_yaml::to_string
                    ▼
crates/operator/src/crds/serialize.rs        (crd_yaml::<K>())
                    │                                          ┌── chart consumers
                    │                                          ▼
                    ├─ ctl `crd` subcommand ──▶ writes ──▶ charts/mikebom-operator/crds/
                    │                                          ▲
                    └─ tests/crd_drift.rs ──▶ asserts equal ───┘  (CI gate)
```

No hand-edited intermediate. CI fails any PR where the chart YAML drifts
from what the generator emits.

## Reconciler

### Lifecycle

```text
main.rs
  ├─ tracing-subscriber.json() — structured JSON log writer
  ├─ Client::try_default()
  ├─ leader::run_with_leadership(client, LeaderConfig{namespace, name, identity}, body)
  │    ├─ acquire `coordination.k8s.io.Lease` (15s duration; ~5s renewal)
  │    └─ on renewal failure → process exits non-zero (k8s restarts pod)
  └─ body = Controller::new(Api::<NamespaceScan>::all(client)).run(reconcile, error_policy, ctx)
        └─ on every watch event + every 5 minutes (periodic resync):
             status::desired_status(spec, now, existing) → patch /status
```

### Status condition vocabulary

The reconciler writes exactly one `condition` per CR with `type=Ready`. Possible
values:

| `status` | `reason`           | Feature | When                                                       |
|----------|--------------------|---------|------------------------------------------------------------|
| `False`  | `InvalidSpec`      | 002     | `spec.target` has empty `namespaces` and unset/empty `labelSelector`. |
| `False`  | `NotYetReconciled` | 002     | Valid spec; orchestration not yet attempted (transient — only seen during the first reconcile cycle in v0.7+). |
| `False`  | `Scanning`         | 007     | Orchestrator ensured ≥1 scan Job per distinct in-scope image (steady state for any CR with active workloads). |
| `False`  | `NoImagesInScope`  | 007     | Valid spec, target resolved to zero pods in phase `Running`/`Pending`. |
| `False`  | `BuildFailed`      | 007     | `scan_job::build_scan_job` rejected the CR's `output` block for at least one in-scope image (e.g., `output.type=Pvc` without `pvc.claimName`). |
| `False`  | `RBACInsufficient` | 007     | Operator lacks `pods:list` in a target namespace, or `jobs:create` in its own namespace. Fail-closed across the CR per constitution III. |

Reasons reserved for future features (do not repurpose):

- `ScanCompleted` (with `status=True`) — feature 008 (Job-status feedback).
- `ScanFailed` — feature 008.

### Requeue cadence

Watch-driven reconciles fire on every CR add/update/delete event. A periodic
`Action::requeue(Duration::from_secs(300))` (5 minutes) acts as a heartbeat —
even idle CRs refresh `lastReconciledAt` at most every 5 minutes so cluster
admins can verify the operator is still alive without a metrics endpoint.

### Idempotency

`status::desired_status` is a pure function of `(spec, now, existing_status)`.
Reconciling an unchanged spec produces an identical condition (only
`lastReconciledAt` advances; the condition's `lastTransitionTime` is preserved).
This is enforced by unit tests in `crates/operator/src/status.rs` that run on
every `cargo test --workspace`.

### Scan-Job builder

`operator::scan_job::build_scan_job(spec, cr_name, image_ref) -> Result<Job, _>`
is the canonical Job-spec entry point. It produces a 3-container `batch/v1.Job`
(`initContainers=[init-pull, mikebom-scan]`, `containers=[output-upload]`)
sharing an `emptyDir` workdir. Feature 002's reconciler skeleton does NOT yet
call this — feature 004+ wires it in. The function is a pure data transform;
unit tests in `crates/operator/src/scan_job/mod.rs` cover every FR from
`specs/003-scan-job-builder/spec.md`, and `e2e/tests/scan_job_dryrun.rs`
validates the produced manifest against a real Kubernetes API server via
`kubectl apply --dry-run=server` (constitution principle VI).

## Security model

- The operator runs with a tightly-scoped `ClusterRole`: read pods,
  namespaces, and workloads; manage Jobs in target namespaces; manage
  `kusari.dev` resources; manage its own leader-election Lease.
- Scan Jobs run under a separate ServiceAccount with no Kubernetes API
  access — they only need to pull the target image and write output.
- Output credentials (S3, OCI) are mounted from `Secret` references in the
  `NamespaceScan` spec, never from operator-global config.

## Testing

### Unit + integration tests

Run via `cargo test --workspace`. The integration test
`crates/operator/tests/crd_drift.rs` enforces constitution principle VII
(Helm Chart Lockstep): it asserts the chart's
`charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml` is byte-equal
to what `operator::crds::serialize::crd_yaml::<NamespaceScan>()` produces.
A second test in the same file pins the generator's determinism.

### Kind-based E2E (gated)

Constitution principle VI requires kind-based E2E coverage for every PR
that touches reconciler logic, Job-template construction, CRD shape, or
RBAC. E2Es live under `e2e/tests/` and are gated behind the
`MIKEBOM_OPERATOR_E2E=1` environment variable so they don't fire during
ordinary `cargo test --workspace` runs.

Local invocation:

```sh
kind create cluster --config e2e/kind-cluster.yaml
MIKEBOM_OPERATOR_E2E=1 cargo test --test crd_install
```

`e2e/tests/crd_install.rs` asserts that `helm install
charts/mikebom-operator/` registers the `NamespaceScan` CRD and that
`kubectl get crd namespacescans.kusari.dev` succeeds within the chart's
60s wait timeout.
