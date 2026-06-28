# Phase 1 — Data Model

This feature touches three data shapes: the `NamespaceScan` status subresource (adds one field), the standard `coordination.k8s.io.Lease` (operator owns one), and the in-process `Ctx` passed to the reconcile function.

## 1. NamespaceScanStatus — additive change

**Location**: `crates/operator/src/crds/namespace_scan.rs`

**Current shape** (bootstrap):

```rust
pub struct NamespaceScanStatus {
    pub conditions: Vec<StatusCondition>,
    pub last_scan_completed_at: Option<String>,
    pub scanned_images: Vec<ScannedImage>,
}
```

**After feature 002**:

```rust
pub struct NamespaceScanStatus {
    pub conditions: Vec<StatusCondition>,
    /// Wallclock of the most recent reconcile attempt; refreshed every reconcile
    /// cycle even when no transition occurred. RFC 3339 string.
    pub last_reconciled_at: Option<String>,
    /// Wallclock of the most recent SUCCESSFUL scan completion (feature 003+).
    /// Distinct from `last_reconciled_at` — reconcile-attempted ≠ scan-completed.
    pub last_scan_completed_at: Option<String>,
    pub scanned_images: Vec<ScannedImage>,
}
```

The new field is **additive** per constitution principle IV. Serde renames `last_reconciled_at` → `lastReconciledAt` via the existing `#[serde(rename_all = "camelCase")]` on the struct. The chart CRD YAML at `charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml` MUST be regenerated; feature 001's `crd_drift.rs` test fails until that happens.

## 2. Condition vocabulary

The reconcile function writes exactly one condition (`type=Ready`) and never more, never fewer:

| condition_type | status | reason                | When set                                                                 |
|----------------|--------|-----------------------|--------------------------------------------------------------------------|
| `Ready`        | `False` | `NotYetReconciled`   | CR has valid target spec; scanning is not yet implemented (this is the steady state for v0.1). |
| `Ready`        | `False` | `InvalidSpec`        | `spec.target.namespaces` is empty AND `spec.target.labelSelector` is unset (FR-011). |

No `True` is written by this feature. Feature 003+ introduces `reason=Scanning` (in-progress) and eventually `Ready=True, reason=ScanCompleted`.

**Idempotency contract**: re-running reconcile against an unchanged CR refreshes `lastReconciledAt` but leaves the condition's `lastTransitionTime` alone — only flipping `status` or `reason` updates `lastTransitionTime` (standard k8s convention).

## 3. Lease (leader election)

The operator owns a `coordination.k8s.io/v1.Lease` named `mikebom-operator-leader` in its own namespace (configured by `MIKEBOM_LEADER_LEASE` env var, default `mikebom-operator-leader`; namespace from `POD_NAMESPACE` env var which the chart injects via the Downward API in T-002).

**Lease shape** (filled by the leader-election helper):

```yaml
apiVersion: coordination.k8s.io/v1
kind: Lease
metadata:
  name: mikebom-operator-leader
  namespace: kusari-operator
spec:
  holderIdentity: mikebom-operator-<pod-name>-<random-suffix>
  leaseDurationSeconds: 15
  acquireTime: "2026-06-27T19:00:00.000000Z"
  renewTime: "2026-06-27T19:00:14.300000Z"
```

`holderIdentity` format: `mikebom-operator-{POD_NAME}` where `POD_NAME` comes from the Downward API. `leaseDurationSeconds: 15` and renewal every ~5s (per kube-rs leader-election convention) gives an upper bound on failover within the SC-003 30s window.

## 4. Reconciler context

In-process data passed to the reconcile function via `kube::runtime::Controller::run(reconcile, error_policy, Arc::new(ctx))`:

```rust
pub struct Ctx {
    pub client: kube::Client,
    pub api: kube::Api<NamespaceScan>,
    pub field_manager: &'static str,  // "mikebom-operator" (for future SSA migration)
}
```

No mutable state. Reconcile pulls the current CR via `api.get(name).await`, computes the desired status, and patches via `api.patch_status(...)`.

## 5. State transitions

The reconcile function is a pure function of the CR's `spec.target` shape:

```text
target.namespaces non-empty OR target.labelSelector set
  → Ready=False, reason=NotYetReconciled, message="scanning not yet implemented"

target.namespaces empty AND target.labelSelector unset
  → Ready=False, reason=InvalidSpec, message="target requires either namespaces or labelSelector"
```

No state transitions across reconciles in this feature — every reconcile re-derives the desired condition from the spec. Feature 003+ adds a state machine when actual scan work begins.

## 6. Validation rules sourced from spec

- **FR-003**: every CR gets a `Ready` condition. Reconcile writes exactly one entry; never appends duplicates.
- **FR-004**: every reconcile updates `lastReconciledAt`. The status patch always includes this field.
- **FR-005 / FR-006**: no `Job` or `Pod` API access. Verified by grepping the implementation for `Api::<Job>` / `Api::<Pod>` — neither appears.
- **FR-007**: leader-election visible as a `Lease`. Verified in T-007's E2E by running `kubectl get lease`.
- **FR-010**: idempotent — reconciling the same spec twice produces identical conditions (only `lastReconciledAt` refreshes). Verified by unit-testing the desired-status computation against an in-memory CR fixture.
- **FR-011**: empty target → `InvalidSpec`. Encoded in the state-transition table above; unit-tested.
- **FR-012**: deleted CR → graceful NotFound. The reconcile function's signature `Result<Action, Error>` lets the controller's error_policy log-and-suppress `Error::NotFound`.
