# Contract: Leader-election Lease

This documents the user-observable shape of the leader-election state, for cluster admins inspecting `kubectl get lease -n kusari-operator`.

## Lease resource

```yaml
apiVersion: coordination.k8s.io/v1
kind: Lease
metadata:
  name: mikebom-operator-leader
  namespace: kusari-operator   # or wherever the operator is deployed
  labels:
    app.kubernetes.io/name: mikebom-operator
spec:
  holderIdentity: mikebom-operator-{POD_NAME}
  leaseDurationSeconds: 15
  acquireTime: "2026-06-27T19:00:00.000000Z"
  renewTime: "2026-06-27T19:00:14.300000Z"
```

## Field semantics

| Field                  | Value                                                                        |
|------------------------|------------------------------------------------------------------------------|
| `metadata.name`        | `mikebom-operator-leader` (configurable via `MIKEBOM_LEADER_LEASE` env)      |
| `metadata.namespace`   | Operator's own namespace (Downward API: `POD_NAMESPACE`)                     |
| `spec.holderIdentity`  | `mikebom-operator-{POD_NAME}` where `POD_NAME` is the Downward-API pod name  |
| `spec.leaseDurationSeconds` | 15 — duration after which a non-renewed lease is considered free          |
| `spec.renewTime`       | Updated every ~5 seconds by the current leader                                |
| `spec.acquireTime`     | Wallclock of the most recent leadership transition                            |

## Configuration

The Helm chart's `values.yaml` already exposes:

```yaml
leaderElection:
  enabled: true
  leaseName: mikebom-operator-leader
```

When `leaderElection.enabled: false`, the operator skips Lease acquisition and reconciles unconditionally. This is **only safe with `replicas: 1`** — operators MUST NOT scale to 2+ replicas with leader-election disabled.

## Observability for cluster admins

```sh
# Inspect current leader
kubectl get lease mikebom-operator-leader -n kusari-operator -o yaml

# Watch leadership transitions during a controlled failover
kubectl get lease mikebom-operator-leader -n kusari-operator -w

# Identify which pod is currently the leader
kubectl get lease mikebom-operator-leader -n kusari-operator \
  -o jsonpath='{.spec.holderIdentity}'
```

## Stability guarantees

- Lease `metadata.name` and namespace location are part of the contract — chart consumers building dashboards/alerts on the Lease can pin to them.
- `holderIdentity` format (`mikebom-operator-{POD_NAME}`) is part of the contract — alert rules can extract pod names from it.
- `leaseDurationSeconds: 15` is the v0.1 default; tunable via Helm `values.yaml` in a future feature without breaking the contract shape.

## SC-003 derivation

`leaseDurationSeconds: 15` + renewal cadence ~5s means: when the leader pod dies, the lease becomes free at most 15s after the last successful renewal. The surviving replica acquires within one polling cycle (~5s). Total: ~20s upper bound, comfortably within SC-003's 30s budget.
