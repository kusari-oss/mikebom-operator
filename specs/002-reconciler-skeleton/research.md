# Phase 0 — Research

Each decision below is a binding input to Phase 1 design.

## R1: Leader-election API surface in kube-rs 0.97

**Decision**: Use the leader-election helpers in `kube::runtime::reflector` (`Lease` / `LeaseLockParams`-style API) if available in 0.97; otherwise, hand-roll a thin wrapper using `kube::Api<coordination::v1::Lease>::patch` with optimistic concurrency on `metadata.resourceVersion`. Implementer verifies which surface 0.97 exposes during T-002 (Foundational); the wrapper lives in `crates/operator/src/leader.rs` regardless.

**Rationale**: The Lease API contract — `holderIdentity`, `leaseDurationSeconds`, `renewTime`, `acquireTime` — is stable; the wrapper around it can be ours even if kube-rs's helpers change shape across minor versions. Putting the wrapper in `leader.rs` insulates the rest of the codebase from kube-rs API drift.

**Alternatives considered**:
- Third-party crates (`leader-elect-rs`, etc.) — extra dep, mostly redundant with what kube-rs provides. Rejected.
- Inline leader-election in `main.rs` — couples bootstrap with leader-election semantics; would force `main.rs` to grow over time. Rejected.

## R2: Structured-log initialization

**Decision**: Initialize `tracing-subscriber` with the `.json()` formatter. Add `"json"` to the `tracing-subscriber` features in the workspace `Cargo.toml` (`features = ["env-filter", "json"]`).

**Rationale**: FR-009 + SC-005 require machine-parseable logs. JSON via `tracing-subscriber` is zero-extra-dep (already in workspace), produces one log line per record, and integrates cleanly with kubectl logs / Loki / Datadog ingestion.

**Alternatives considered**:
- `bunyan-style` via a third-party formatter — extra dep, no advantage over native JSON. Rejected.
- Keep the current human-format default and convert in a sidecar — operationally ugly; rejected.

## R3: Status update strategy

**Decision**: Use `Patch::Merge` against the `/status` subresource for v0.1. The reconciler reads the CR, computes the new conditions + `lastReconciledAt`, and applies a Merge patch with a JSON body that includes only the keys it owns.

**Rationale**: Server-Side Apply (SSA) is the right long-term answer when multiple controllers manage overlapping fields, but for v0.1 there's exactly one writer (this operator) for `NamespaceScanStatus`. Merge is simpler (no field-manager registration, no apply-vs-update divergence). Migrate to SSA when a second status writer enters the picture (e.g., a future status-aggregator controller).

**Alternatives considered**:
- `Patch::Apply` (SSA) from day one — premature; introduces field-manager bookkeeping for no observable benefit at v0.1.
- `Resource::replace` on the full CR — race-prone; rejected.

## R4: Requeue cadence

**Decision**: Default `Action::requeue(Duration::from_secs(300))` — 5-minute periodic resync — combined with watch-driven reconciles. `lastReconciledAt` thus refreshes at most every 5 min when nothing else changes.

**Rationale**: kube-rs `Controller` reconciles on every watch event by default (changes are picked up immediately), but a periodic resync catches transient API failures and serves as a heartbeat signal. 5 minutes is long enough not to spam the API but short enough that "operator is alive" is observable via the timestamp without metrics. Tunable via env var in a future feature; hardcoded for v0.1.

**Alternatives considered**:
- No requeue (only watch events) — heartbeat never refreshes when the CR is steady; rejected for v0.1's observability story.
- 30s requeue — too noisy; rejected.
- 1h requeue — too sparse; rejected.

## R5: Operator-side spec validation locus (FR-011)

**Decision**: Validation lives inside the reconcile function. The reconciler short-circuits on an invalid spec by writing `Ready=False, reason=InvalidSpec, message=<which field>` and returning a long requeue (5 min). No webhook in this feature.

**Rationale**: A webhook would catch invalid specs at admission time but adds operational complexity (cert management, TLS, webhook deployment) that's out of scope for v0.1. Reconciler-side validation is universal — every controller does it as a defense-in-depth measure regardless of webhook presence. Webhooks can be added later (post-v0.1 per spec out-of-scope) without changing the reconciler's validation logic.

**Alternatives considered**:
- Add a webhook now — scope creep; rejected.
- Validate via CRD `x-kubernetes-validations` (CEL) — only catches simple cases; we'd still need reconciler-side validation for cross-field rules; rejected for v0.1.

## R6: Kind E2E shape

**Decision**: `e2e/tests/reconciler_skeleton.rs` (gated by `MIKEBOM_OPERATOR_E2E=1`) is one test function that:
1. Installs the chart fresh in a kind cluster (helm install, wait 60s).
2. Asserts the Lease object exists in the operator namespace with non-empty `holderIdentity`.
3. Applies `examples/namespacescan.yaml`.
4. Polls `kubectl get namespacescan scan-prod -o jsonpath='{.status.conditions[?(@.type=="Ready")].reason}'` until it's `NotYetReconciled` (timeout 10s).
5. Asserts `status.lastReconciledAt` is non-empty and within 10s of "now."
6. Cleans up.

**Rationale**: Single E2E covers US1 (chart install → operator Ready), US2 (CR apply → status acknowledged), and observability of US3's Lease without requiring pod-kill timing tests. Pod-kill timing for US3's full failover assertion is fragile; defer to a separate test or run-by-hand exercise.

**Alternatives considered**:
- Three separate E2Es (one per user story) — more files, slower cumulative wall-clock, harder cleanup. Rejected.
- Skip the kind E2E and rely on `crd_install.rs` from feature 001 — that test doesn't apply a CR or observe reconcile; doesn't cover this feature's scope. Rejected.

## R7: RBAC verification

**Decision**: Confirm during T-001 (Setup) that the chart's existing `ClusterRole` (from feature 001's `charts/mikebom-operator/templates/rbac.yaml`) already grants:
- `kusari.dev` namespacescans (get/list/watch/update/patch — for spec + status subresource)
- `coordination.k8s.io` leases (get/list/watch/create/update/patch/delete — for leader-election)
- `""` events (create/patch — for k8s Events)

If any verb is missing, the implementation task includes a chart RBAC update (which doesn't trigger feature 001's drift check — that only watches the CRD YAML).

**Rationale**: Feature 001's chart already has these verbs (I checked at bootstrap time), but a Phase-0 verification step keeps planning honest. If verbs are missing, we add them in T-001 rather than discovering at T-007's E2E run.

**Alternatives considered**:
- Skip verification and let E2E catch missing verbs — costs a kind-E2E cycle per missing verb; rejected.

## R8: `lastReconciledAt` serialization

**Decision**: Serialize as `String` in `NamespaceScanStatus` with content RFC 3339 (`chrono::Utc::now().to_rfc3339()`). Field is `Option<String>` to keep it additive; freshly-installed operators against pre-existing CRs report `None` until first reconcile.

**Rationale**: Kubernetes convention for status timestamps is RFC 3339 strings (`metav1.Time` underneath, but exposed as string in JSON). Matches the existing `last_scan_completed_at` field's shape exactly — no special-case parsing on the consumer side.

**Alternatives considered**:
- Native `metav1.Time` via k8s-openapi — adds a dep on the k8s-openapi `Time` import path for one field; rejected (the existing field is already a String, consistency wins).
- Unix epoch milliseconds — non-standard for k8s; rejected.
