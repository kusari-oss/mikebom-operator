# Contract: NamespaceScan status (additions from feature 002)

This documents the user-facing contract for what `kubectl get namespacescan -o yaml` produces after feature 002 lands. It supersedes nothing — feature 001's CRD reference still holds; this is additive.

## New field

```yaml
status:
  lastReconciledAt: "2026-06-27T19:00:14Z"   # NEW in feature 002
```

- Type: RFC 3339 timestamp string (UTC, second-precision).
- Optional: absent until the operator has reconciled the CR at least once.
- Semantics: wallclock of the most recent reconcile attempt, regardless of whether the reconcile resulted in a status transition. Updates on every reconcile cycle (watch event + 5-minute periodic resync).
- Not to be confused with `status.lastScanCompletedAt`, which only updates when a scan actually completes (feature 003+).

## Condition vocabulary written by this feature

The reconciler writes exactly one `condition` per CR with `type=Ready`. Possible values for this feature:

```yaml
status:
  conditions:
    - type: Ready
      status: "False"
      reason: NotYetReconciled
      message: "Scanning not yet implemented; mikebom-operator feature 003 introduces the Job spec."
      lastTransitionTime: "2026-06-27T19:00:14Z"
```

OR:

```yaml
status:
  conditions:
    - type: Ready
      status: "False"
      reason: InvalidSpec
      message: "spec.target requires either namespaces or labelSelector"
      lastTransitionTime: "2026-06-27T19:00:14Z"
```

`lastTransitionTime` follows standard Kubernetes convention: only updates when `status` or `reason` changes. Heartbeat-style observability comes from `lastReconciledAt`, not `lastTransitionTime`.

## Reason values reserved for future features

| reason                | Introduced in |
|-----------------------|---------------|
| `NotYetReconciled`    | feature 002 (this feature) |
| `InvalidSpec`         | feature 002 (this feature) |
| `Scanning`            | feature 003 (Job pod template) |
| `ScanFailed`          | feature 003 |
| `ScanCompleted`       | feature 003 (with status=True) |
| `RBACInsufficient`    | feature 003+ (per constitution III) |

Reserving these names now so contributors don't pick conflicting reasons in concurrent feature work.

## Stability guarantees

- The `lastReconciledAt` field is part of `v1alpha1` once feature 002 ships. Removing it would be a breaking change requiring `v1alpha2` per constitution IV.
- Condition reason values are part of the contract. New reasons can be added; existing reasons cannot be repurposed (e.g., `NotYetReconciled` cannot later mean "operator is overloaded").
- `lastTransitionTime` semantics follow upstream Kubernetes convention — we don't override.

## What consumers can rely on

- A `kubectl wait --for=condition=Ready namespacescan/<name>` will hang indefinitely (and that's correct) until a feature post-002 actually completes a scan — the skeleton intentionally never sets `Ready=True`.
- A `kubectl get namespacescan -o jsonpath='{.status.lastReconciledAt}'` returns a fresh RFC 3339 string every ≤ 5 minutes when the operator is healthy, regardless of CR change rate.
- Multiple operator replicas running with leader election still produce single-writer behavior on this field — no interleaved updates from the non-leader.
