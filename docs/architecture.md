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
