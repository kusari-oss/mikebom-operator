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
| `crates/operator/` | reconciler binary (`mikebom-operator`) |
| `crates/ctl/` | optional debug CLI (`mikebom-operator-ctl`) |
| `charts/mikebom-operator/` | Helm chart, versioned independently |
| `e2e/` | kind-based end-to-end test harness |
| `examples/` | sample `NamespaceScan` CRs |
