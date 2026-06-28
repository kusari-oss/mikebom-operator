# Quickstart: NamespaceScan reconciler skeleton

## For cluster admins installing the operator

```sh
# 1. Install the chart (CRD ships in chart, operator deployment too)
helm install mikebom-operator charts/mikebom-operator \
  -n kusari-operator --create-namespace --wait --timeout 60s

# 2. Verify operator is running and acquired leadership
kubectl get pods -n kusari-operator
kubectl get lease mikebom-operator-leader -n kusari-operator -o yaml
# Expect: spec.holderIdentity matches the operator pod name

# 3. Apply a NamespaceScan CR and watch the operator acknowledge it
kubectl apply -f examples/namespacescan.yaml
kubectl get namespacescan scan-prod -o yaml -n kusari-operator -w
# Expect within 10s:
#   status.conditions[0].type: Ready
#   status.conditions[0].status: "False"
#   status.conditions[0].reason: NotYetReconciled
#   status.lastReconciledAt: <RFC 3339 timestamp within 10s of now>
```

The `Ready=False` is correct — actual scanning lands in feature 003. This feature is the proof that the operator wires up cleanly.

## For contributors testing the reconciler

```sh
# 1. Unit tests for the desired-status computation
cargo test --workspace --test reconcile_namespace_scan

# 2. Drift check (will fail before T-006's chart YAML regen)
cargo test --workspace --test crd_drift

# 3. Kind-based E2E (gated)
kind create cluster --config e2e/kind-cluster.yaml
MIKEBOM_OPERATOR_E2E=1 cargo test --test reconciler_skeleton
```

## For debugging reconcile failures

Logs are structured JSON. To pretty-print and filter:

```sh
# Tail the operator's reconcile decisions
kubectl logs -n kusari-operator deployment/mikebom-operator -f | \
  jq 'select(.fields.reconcile)'

# Find error-level entries from the last 10 minutes
kubectl logs -n kusari-operator deployment/mikebom-operator --since=10m | \
  jq 'select(.level == "ERROR")'
```

Useful filters:

| Field                     | What to filter for                                  |
|---------------------------|-----------------------------------------------------|
| `level`                   | `INFO` / `WARN` / `ERROR`                           |
| `fields.namespace_scan`   | The CR name (when present)                          |
| `fields.event`            | `startup` / `leader_acquired` / `reconcile`         |
| `fields.reason`           | `NotYetReconciled` / `InvalidSpec` (this feature)   |

## Common failure modes

| Symptom                                            | Probable cause                                                                                                          | Fix                                                                                                |
|----------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------|----------------------------------------------------------------------------------------------------|
| Operator pod stuck in `CrashLoopBackOff`           | RBAC missing for `coordination.k8s.io.leases`                                                                           | Verify the chart's `rbac.yaml` includes the lease verbs; re-install with the updated chart.        |
| `status` never populates after CR apply            | Pod not the leader; Lease holderIdentity is empty or wrong                                                              | `kubectl get lease ...`; if no holder, check operator logs for Lease patch errors.                 |
| Drift test fails locally after editing the CRD struct | Forgot to regenerate the chart CRD YAML                                                                                | `cargo run --bin mikebom-operator-ctl -- crd --output charts/.../namespacescan.kusari.dev_v1.yaml` |
| `lastReconciledAt` doesn't refresh in steady state | Operator not the leader, OR reconcile is panicking silently                                                              | Check Lease holderIdentity; check logs for panic messages.                                          |
| 2-replica deployment shows two pods reconciling     | Leader election disabled (`leaderElection.enabled: false` in values)                                                    | Re-enable in `values.yaml`; operators MUST NOT run replicas >1 with leader-election off.            |

## Performance expectations

| Operation                              | Budget (per spec SCs) |
|----------------------------------------|-----------------------|
| Helm install → operator Ready          | < 30s                 |
| CR apply → `status.lastReconciledAt`   | < 10s                 |
| Leader pod kill → new leader + reconcile | < 30s               |
| Steady-state reconcile latency (1 CR)  | < 1s                  |
| 24h uptime without OOM / log explosion | required (SC-004)     |
