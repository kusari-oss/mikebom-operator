# mikebom-operator

Kubernetes operator that watches namespaces and generates an SBOM for each
running container image via [mikebom](https://github.com/kusari-oss/mikebom).

Status: pre-alpha. See [architecture.md](architecture.md) for the design and
[crd-reference.md](crd-reference.md) for the `NamespaceScan` CRD.

## Quickstart

```sh
kind create cluster --config e2e/kind-cluster.yaml
helm install mikebom-operator charts/mikebom-operator \
  -n kusari-operator --create-namespace
kubectl apply -f examples/namespacescan.yaml
kubectl get namespacescan -o yaml
```

## Layout

| Path | Purpose |
|---|---|
| `crates/operator/` | reconciler library + binary (`mikebom-operator`) |
| `crates/ctl/` | debug CLI (`mikebom-operator-ctl`) — includes the CRD generator |
| `charts/mikebom-operator/` | Helm chart, versioned independently; CRD YAML is generated |
| `e2e/` | kind-based end-to-end test harness (gated behind `MIKEBOM_OPERATOR_E2E=1`) |
| `examples/` | sample `NamespaceScan` CRs |

## Editing the `NamespaceScan` CRD

The chart's CRD manifest is generated from the Rust struct at
`crates/operator/src/crds/namespace_scan.rs`. After editing the struct:

```sh
cargo run --bin mikebom-operator-ctl -- crd \
  --output charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml
cargo test --workspace
```

CI's `cargo test --workspace` runs a drift check that asserts the chart
YAML matches the in-process generator. If a PR changes the struct but not
the chart YAML (or vice versa), the build fails with a diagnostic naming
the regen command verbatim. See [architecture.md — Testing](architecture.md#testing).
