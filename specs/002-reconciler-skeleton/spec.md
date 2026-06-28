# Feature Specification: NamespaceScan reconciler skeleton

**Feature Branch**: `002-reconciler-skeleton`

**Created**: 2026-06-27

**Status**: Draft

**Input**: User description: "Minimal NamespaceScan reconciler skeleton: kube::runtime::Controller wired for NamespaceScan, leader election via kube::runtime::reflector::Lease, and a reconcile function that acknowledges the CR by setting Ready=False with Reason=NotYetReconciled and updating lastReconciledAt on status. No pod enumeration, no Job creation."

## Clarifications

### Session 2026-06-27

- Q: How should the reconcile timestamp surface in the CRD status? → A: Add a new `status.lastReconciledAt` field (RFC 3339 string), additive to `v1alpha1` per constitution IV. The existing `status.lastScanCompletedAt` stays reserved for scan completion in feature 003+. No `observedGeneration` for this feature (deferred until it carries information).

## User Scenarios & Testing *(mandatory)*

### User Story 1 — Operator becomes Ready in-cluster (Priority: P1)

A cluster admin installs the Helm chart into a fresh Kubernetes cluster. The operator pod starts, establishes leadership, and reaches the Kubernetes `Ready` state. The cluster admin can verify by reading the pod's structured logs that the operator is healthy and watching for `NamespaceScan` resources.

**Why this priority**: Without this, nothing else in the operator works. P1 because the chart-installed-operator-runs check is the foundation for every later feature. This is the minimal user-visible signal that the operator exists and is alive.

**Independent Test**: `helm install` the chart → observe the operator pod reaches `Ready` within the chart's `--wait --timeout` window → `kubectl logs <operator-pod>` shows a startup line followed by a leadership-acquired line. Delivers value even without US2/US3 because it proves the chart and operator binary are mutually consistent.

**Acceptance Scenarios**:

1. **Given** a fresh kind cluster with the chart not yet installed, **When** the admin runs `helm install mikebom-operator charts/mikebom-operator -n kusari-operator --create-namespace --wait --timeout 60s`, **Then** the install command exits 0 and the operator Deployment reports 1/1 ready replicas.
2. **Given** the operator pod is running, **When** the admin runs `kubectl logs <operator-pod> -n kusari-operator`, **Then** the log stream contains at least one record indicating successful startup and one indicating leadership acquisition, both in a structured (machine-parseable) format.

---

### User Story 2 — Reconciler acknowledges NamespaceScan CRs (Priority: P2)

A cluster admin applies a `NamespaceScan` CR to the cluster. The operator observes the new resource and updates its `status` to communicate that the operator has seen the CR but has not yet performed any scanning work. The admin can see this acknowledgement by reading the CR's status.

**Why this priority**: This is the visible proof that the controller loop is wired correctly. Without it, US1 (operator alive) gives no signal about whether the reconcile pipeline functions. Marked P2 because it depends on US1 but is independently testable once US1 lands.

**Independent Test**: With the operator installed and Ready, apply `examples/namespacescan.yaml` → within a bounded window, `kubectl get namespacescan scan-prod -o yaml -n kusari-operator` shows a `status.conditions[]` entry with `type=Ready`, `status=False`, and `reason=NotYetReconciled`, plus a `status.lastReconciledAt` timestamp.

**Acceptance Scenarios**:

1. **Given** the operator is running with no existing `NamespaceScan` resources, **When** the admin applies a valid `NamespaceScan` manifest, **Then** within 10 seconds the resource's `status.conditions[]` contains an entry with `type=Ready`, `status=False`, and a `reason` value that communicates the not-yet-scanned state.
2. **Given** a `NamespaceScan` CR exists and has been acknowledged by the operator, **When** the admin applies a modified version of the same CR (e.g., changes the schedule), **Then** the operator re-runs reconcile and updates `lastReconciledAt` to a newer timestamp while keeping the same `Ready=False` condition.
3. **Given** a `NamespaceScan` CR has been acknowledged, **When** the admin deletes the CR, **Then** the operator does not error and the CR is removed cleanly (no stuck finalizers — feature 002 does not add finalizers).

---

### User Story 3 — Multi-replica safety via leader election (Priority: P3)

A site reliability engineer scales the operator Deployment to 2+ replicas for availability. Only one replica actively reconciles `NamespaceScan` resources at any moment. When the active replica fails, another replica takes over leadership and continues processing within a bounded window.

**Why this priority**: This is plumbing that pays off only at scale. Most installs run a single replica (the chart's default); leader election is invisible there. Marked P3 because it's a correctness guarantee that becomes user-visible only when the user deliberately scales out.

**Independent Test**: Scale the operator Deployment to `replicas: 2`. Observe the `Lease` object in the operator namespace — exactly one replica holds the lease. Delete the leader pod; observe that within 30 seconds the other replica acquires the lease and resumes reconciling (status timestamps continue to update on existing CRs).

**Acceptance Scenarios**:

1. **Given** the operator Deployment is scaled to 2 replicas, **When** the admin inspects the leader-election `Lease` resource in the operator namespace, **Then** the `Lease.spec.holderIdentity` matches exactly one of the two operator pod identifiers.
2. **Given** a 2-replica deployment with a current leader, **When** the admin deletes the leader pod, **Then** within 30 seconds a new leader is established, the Lease's `holderIdentity` changes to the surviving replica, and reconciliation of any existing `NamespaceScan` CRs resumes (verifiable by a fresh `lastReconciledAt` on a CR within a further 30 seconds).

---

### Edge Cases

- Operator restarts mid-reconcile: on restart, the controller resyncs the watch cache and re-reconciles every existing `NamespaceScan` CR. Status updates are idempotent — re-running reconcile against an already-acknowledged CR refreshes the `lastReconciledAt` timestamp without flapping the condition.
- `NamespaceScan` CR deleted while the operator is mid-reconcile: the reconciler treats the resulting `NotFound` response as a non-error terminal state for that CR and does not retry.
- Cluster has zero `NamespaceScan` CRs: the operator runs idle, holds the leader-election lease, and emits periodic structured heartbeat-style logs (no errors).
- Status update fails with a conflict (another writer raced): the controller's standard backoff requeues the reconcile; the error is logged at `info` (not `error`) level to avoid alert noise.
- `NamespaceScan` CR with both `target.namespaces` and `target.labelSelector` empty: the reconciler updates the condition with `reason=InvalidSpec` and a message naming the offending fields. (Validating webhooks are post-v0.1; this is operator-side defense.)

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The operator pod, when installed via the Helm chart, MUST reach Kubernetes `Ready` state within the chart's `--wait` timeout (default 60s).
- **FR-002**: The operator MUST acquire leader-election leadership before reconciling any `NamespaceScan` CR.
- **FR-003**: For every `NamespaceScan` CR present in the cluster (whether already-existing at startup or newly created), the operator MUST write a `status.conditions[]` entry with `type=Ready`, `status=False`, and a reason value that communicates the not-yet-scanned state (e.g., `NotYetReconciled`).
- **FR-004**: On every reconcile cycle, the operator MUST write or update a status field that records the timestamp of the most recent reconcile attempt for that CR.
- **FR-005**: The operator MUST NOT create, update, or delete any `batch/v1.Job` resources in this feature.
- **FR-006**: The operator MUST NOT enumerate `Pod` resources in any namespace in this feature.
- **FR-007**: When multiple operator replicas are running, at most one MUST be actively reconciling `NamespaceScan` resources at any moment. The mechanism MUST be observable by reading a Kubernetes `Lease` object owned by the operator.
- **FR-008**: When the active leader replica becomes unavailable, another replica MUST acquire leadership and resume reconciling within 30 seconds.
- **FR-009**: The operator MUST emit machine-parseable structured logs (JSON or equivalent) for at minimum: startup, leadership acquisition, leadership release, each reconcile cycle (including which CR and the outcome), and any error conditions.
- **FR-010**: The operator's reconcile MUST be idempotent — running reconcile twice in succession against an unchanged CR MUST NOT cause status flapping (no condition transitions between identical states beyond timestamp refresh).
- **FR-011**: When a `NamespaceScan` CR has both `target.namespaces` empty and `target.labelSelector` empty, the operator MUST report this via the `Ready=False` condition with a reason value that communicates the validation failure (e.g., `InvalidSpec`).
- **FR-012**: When a `NamespaceScan` CR is deleted, the operator MUST handle the resulting `NotFound` response without logging an error and without leaving stuck finalizers (feature 002 adds no finalizers).

### Key Entities

- **NamespaceScan CR**: Defined in feature 001. This feature adds writes to its `status` subresource (already declared in the CRD).
- **NamespaceScanStatus.conditions**: Existing field. This feature populates at least one entry with `type=Ready`, `status=False`.
- **NamespaceScanStatus.lastReconciledAt**: A new optional RFC 3339 string field added to `NamespaceScanStatus` in `v1alpha1` (additive change per constitution IV). Records the wallclock of the most recent reconcile attempt; refreshed on every reconcile cycle even when no status transition occurred. Distinct from `lastScanCompletedAt`, which is reserved for actual scan completion in feature 003+.
- **Leader-election Lease**: A Kubernetes `coordination.k8s.io/v1.Lease` resource owned by the operator in its own namespace. The Lease's `spec.holderIdentity` names the active reconciling replica.
- **Operator pod**: The running operator process. This feature does not add new container args/env beyond what the Helm chart already wires.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A fresh `helm install` of the chart into a kind cluster produces an operator pod that reaches `Ready` state within 30 seconds (well inside the chart's default 60s `--wait` timeout).
- **SC-002**: After applying a `NamespaceScan` CR, its `status.conditions[type=Ready]` field is populated with `status=False` and a not-yet-scanned reason within 10 seconds, and `status.lastReconciledAt` is set to a timestamp within 5 seconds of the apply time.
- **SC-003**: In a 2-replica deployment, killing the leader pod results in a new leader and resumed reconcile activity within 30 seconds (observed via `Lease.spec.holderIdentity` change AND a refreshed `lastReconciledAt` on an existing CR).
- **SC-004**: The operator runs continuously for 24 hours without exhibiting OOM kills, leader-election flapping (more than one acquire/release transition per hour under steady state), or unbounded log volume growth.
- **SC-005**: 100% of log records emitted by the operator parse as structured records (JSON or the chosen format) with no plain-text fallback lines.

## Assumptions

- The operator runs with a single replica by default in the Helm chart (`replicas: 1` in `values.yaml`). Operators wanting HA scale to 2+ replicas explicitly; the leader-election plumbing is in place regardless.
- The Helm chart's RBAC already grants what's needed: get/list/watch/update/patch on `kusari.dev` `NamespaceScans` (status subresource included), full access to `coordination.k8s.io.Leases` in the operator's namespace, and `create/patch` on `Events`. (Verified in `charts/mikebom-operator/templates/rbac.yaml` from feature 001.)
- Structured logging format is JSON. The existing `RUST_LOG`/`tracing-subscriber` wiring in the operator binary already supports this.
- The `lastReconciledAt` timestamp is a **new** optional field on `NamespaceScanStatus`, added in this feature (Clarifications Q1 → A). The existing `lastScanCompletedAt` is reserved for actual scan completion in feature 003+. The CRD shape change is additive per constitution principle IV; the chart's `namespacescan.kusari.dev_v1.yaml` MUST be regenerated via the feature 001 generator + drift check.
- Metrics endpoint (Prometheus `/metrics`) is NOT part of this feature — it's a separate observability milestone.
- Webhook-based admission validation is NOT part of this feature; operator-side validation per FR-011 is the only defense for v0.1.

## Out of scope *(intentional deferral)*

- Pod enumeration in target namespaces (feature 007 / image-diff scope).
- Job creation and the 3-container scan choreography (feature 003).
- Output-backend integration: PVC (004), S3 (005), OCI (006).
- Multi-cluster operation.
- Validation/mutating webhooks.
- Prometheus metrics endpoint.
- Finalizers on `NamespaceScan` CRs (added when feature 003's Jobs need cleanup-on-CR-delete).
