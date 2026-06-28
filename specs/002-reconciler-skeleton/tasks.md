---

description: "Task list for feature 002-reconciler-skeleton"
---

# Tasks: NamespaceScan reconciler skeleton

**Input**: Design documents from `/specs/002-reconciler-skeleton/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/namespacescan-status.md, contracts/leader-election.md, quickstart.md

**Tests**: The spec mandates test artifacts — FR-009/SC-005 (structured logs) verifiable only via E2E; FR-010 (idempotency) + FR-011 (InvalidSpec) verifiable via unit tests on `desired_status`; constitution VI requires a kind-based E2E for reconciler-logic changes. Test tasks here are **deliverables**, not optional.

**Organization**: Tasks are grouped by user story (US1 → US2 → US3 in priority order). US2 and US3 both extend the same `e2e/tests/reconciler_skeleton.rs` file authored in US1, so within-phase work is largely sequential; US3's separate failover test (T017) can run in parallel with US3's Lease-observability extension (T016).

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: User story label (US1, US2, US3)
- Paths are repo-relative.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Workspace deps + chart Deployment env-vars + RBAC pre-flight.

- [ ] T001 [P] Add `"json"` to `tracing-subscriber` features in workspace `Cargo.toml` so the operator binary can initialize structured JSON logs per FR-009 / SC-005 (resulting line: `tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }`)
- [ ] T002 [P] Add Downward-API env vars to `charts/mikebom-operator/templates/deployment.yaml`: `POD_NAME` from `metadata.name` and `POD_NAMESPACE` from `metadata.namespace`. The operator binary reads these to derive `holderIdentity` and the Lease's namespace per contracts/leader-election.md.
- [ ] T003 Verify `charts/mikebom-operator/templates/rbac.yaml` already grants the verbs the reconciler skeleton needs (per research §R7): `kusari.dev` `namespacescans` + `namespacescans/status` (get/list/watch/update/patch), `coordination.k8s.io` `leases` (get/list/watch/create/update/patch/delete), `""` `events` (create/patch). If anything is missing, add it.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Add the CRD field, regenerate the chart YAML, and build out the reconciler library code so all user-story phases can layer E2E tests on top.

**⚠️ CRITICAL**: No user-story phase can start until this phase is complete.

- [ ] T004 Add `pub last_reconciled_at: Option<String>` to `NamespaceScanStatus` in `crates/operator/src/crds/namespace_scan.rs` (positioned before `last_scan_completed_at` to keep the natural lifecycle order). Update the doc-comment per data-model §1.
- [ ] T005 Regenerate the chart's CRD YAML: `cargo run --bin mikebom-operator-ctl -- crd --output charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml` (depends on T004 — without T004 the regen would output the old shape).
- [ ] T006 Run `cargo test --test crd_drift` and confirm both `chart_crd_yaml_matches_generator` and `generator_is_deterministic` pass against the new YAML.
- [ ] T007 Implement `crates/operator/src/leader.rs` exposing `pub async fn run_with_leadership<F, Fut>(client: kube::Client, lease_namespace: String, lease_name: String, identity: String, body: F) -> anyhow::Result<()>` that acquires a `coordination.k8s.io/v1.Lease` (15s duration, ~5s renewal per contracts/leader-election.md) and only runs `body()` while leader. Per research §R1 — verify the exact kube-rs 0.97 surface at impl time.
- [ ] T008 Implement `crates/operator/src/status.rs` with `pub fn desired_status(spec: &NamespaceScanSpec, now: chrono::DateTime<chrono::Utc>, existing: Option<&NamespaceScanStatus>) -> NamespaceScanStatus`. Include `#[cfg(test)] mod tests` covering: (a) valid spec → single `Ready=False, reason=NotYetReconciled` condition + `lastReconciledAt` populated; (b) empty target → `reason=InvalidSpec`; (c) idempotency: calling twice with identical inputs produces conditions whose `lastTransitionTime` is the same as the existing one (only `lastReconciledAt` advances); (d) transition: when reason changes, `lastTransitionTime` updates. Satisfies FR-010, FR-011.
- [ ] T009 Implement `crates/operator/src/reconcile/namespace_scan.rs` with the `reconcile(obj: Arc<NamespaceScan>, ctx: Arc<Ctx>) -> Result<Action, Error>` + `error_policy(_: Arc<NamespaceScan>, err: &Error, _: Arc<Ctx>) -> Action` functions and the `Ctx` + `Error` types. Reconcile calls `status::desired_status`, applies a `Patch::Merge` against the `/status` subresource via `Api::patch_status`, and returns `Action::requeue(Duration::from_secs(300))` per research §R3 + §R4.
- [ ] T010 Wire `crates/operator/src/main.rs`: initialize `tracing_subscriber::fmt().json().with_env_filter(...)`; emit a `tracing::info!(event = "startup", ...)` line; `kube::Client::try_default().await?`; read `POD_NAMESPACE` + `POD_NAME` env vars; call `leader::run_with_leadership(...)` wrapping `Controller::new(Api::<NamespaceScan>::all(client), watcher::Config::default()).run(reconcile::namespace_scan::reconcile, reconcile::namespace_scan::error_policy, Arc::new(Ctx { ... })).for_each(|_| async {}).await`.
- [ ] T011 Run `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` and resolve anything that fires before moving to user-story phases.

**Checkpoint**: CRD shape updated + chart in sync; reconciler library compiles + unit tests pass; main.rs ready to deploy.

---

## Phase 3: User Story 1 — Operator becomes Ready in-cluster (Priority: P1) 🎯 MVP

**Goal**: `helm install` produces a `Ready` operator pod within 30 seconds; operator logs show structured-JSON startup + leadership-acquired records.

**Independent Test**: Install the chart fresh in a kind cluster, observe the operator pod reaches `Ready` within 30s, run `kubectl logs ... -o json | jq '.fields.event'` and observe both `"startup"` and `"leader_acquired"`.

### Implementation for User Story 1

- [ ] T012 [US1] Create `e2e/tests/reconciler_skeleton.rs` with a `helm_install_makes_operator_ready` test, gated by `MIKEBOM_OPERATOR_E2E=1`. The test: (a) cleans up prior installs/namespace best-effort; (b) runs `helm install mikebom-operator charts/mikebom-operator -n kusari-operator --create-namespace --kube-context kind-mikebom-operator-e2e --wait --timeout 30s`; (c) asserts install succeeded; (d) `kubectl logs deployment/mikebom-operator -n kusari-operator` returns structured JSON records and at least one carries `event=startup` and at least one carries `event=leader_acquired`. Satisfies FR-001, FR-002, FR-009, SC-001.

**Checkpoint**: Operator installs and runs in-cluster. No reconcile work happens yet (no CRs applied) — that's US2.

---

## Phase 4: User Story 2 — Reconciler acknowledges NamespaceScan CRs (Priority: P2)

**Goal**: Apply a `NamespaceScan` CR; within 10 seconds its `status.conditions[Ready=False, reason=NotYetReconciled]` and `status.lastReconciledAt` are populated.

**Independent Test**: With the operator installed (US1 done), `kubectl apply -f examples/namespacescan.yaml` and poll `kubectl get namespacescan scan-prod -o jsonpath='{.status.conditions[?(@.type=="Ready")].reason}'` until it equals `NotYetReconciled` (within 10s); separately verify `lastReconciledAt` is non-empty.

**Depends on**: Phase 3 (T012 must have created `reconciler_skeleton.rs`; US2 tasks extend the same file).

### Implementation for User Story 2

- [ ] T013 [US2] Extend `e2e/tests/reconciler_skeleton.rs` with a `reconciler_acknowledges_valid_cr` test that (after US1's install) applies `examples/namespacescan.yaml`, polls `kubectl get namespacescan scan-prod -o jsonpath=...` until `.status.conditions[?(@.type=="Ready")].reason` equals `NotYetReconciled` (timeout 10s), and asserts `.status.lastReconciledAt` is a non-empty RFC 3339 string. Satisfies FR-003, FR-004, SC-002.
- [ ] T014 [US2] Extend `e2e/tests/reconciler_skeleton.rs` with a `reconciler_flags_invalid_spec` test that applies a `NamespaceScan` CR with `spec.target.namespaces: []` and `spec.target.labelSelector` unset, and polls for `.status.conditions[?(@.type=="Ready")].reason == "InvalidSpec"` within 10s. Satisfies FR-011.
- [ ] T015 [US2] Extend `e2e/tests/reconciler_skeleton.rs` with a `reconciler_handles_cr_deletion` test that deletes the CR applied in T013 and asserts no JSON log records with `level=ERROR` appear in the operator's logs during the 30s following deletion. Satisfies FR-012.

**Checkpoint**: Reconciler is observably alive — CRs get acknowledged, invalid specs are flagged, deletions are clean. The MVP-plus-enforcement state.

---

## Phase 5: User Story 3 — Multi-replica safety via leader election (Priority: P3)

**Goal**: With multi-replica deployments, exactly one replica reconciles at a time; failover happens within 30 seconds of leader pod death.

**Independent Test**: T016 is observable in any install (single replica still produces a Lease with the pod as holder). T017 requires deliberate scale + pod-kill — gated behind a separate env var so it's opt-in.

**Depends on**: Phase 3 (chart installed) + Phase 4 (CR exists for T017's reconcile-resumption assertion).

### Implementation for User Story 3

- [ ] T016 [P] [US3] Extend `e2e/tests/reconciler_skeleton.rs` with a `leader_election_lease_visible` test that asserts a `coordination.k8s.io/v1.Lease` named `mikebom-operator-leader` exists in `kusari-operator` namespace and its `.spec.holderIdentity` is non-empty and starts with `mikebom-operator-` (matching the format from contracts/leader-election.md). Satisfies FR-007.
- [ ] T017 [P] [US3] Add `e2e/tests/reconciler_failover.rs` (separate file from `reconciler_skeleton.rs`) gated by `MIKEBOM_OPERATOR_E2E_FAILOVER=1` (NOT the same as `MIKEBOM_OPERATOR_E2E`) with a `failover_within_30s` test that: (a) scales the operator Deployment to `replicas: 2` via `kubectl scale`; (b) waits for the Lease `holderIdentity` to settle on one of the two pods; (c) `kubectl delete pod <leader>`; (d) polls the Lease for a new `holderIdentity` distinct from the killed pod within 30s; (e) polls an existing CR's `status.lastReconciledAt` to confirm reconcile resumes within a further 30s. Satisfies FR-008, SC-003. Gated separately because pod-kill is slow and flakier than the steady-state E2E.

**Checkpoint**: Multi-replica HA is exercised. v0.1 reconciler skeleton is complete.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Documentation updates + the pre-PR gate.

- [ ] T018 [P] Add a "Reconciler" subsection to `docs/architecture.md` covering: the condition vocabulary (NotYetReconciled, InvalidSpec, plus the reasons reserved for features 003+ per contracts/namespacescan-status.md), the requeue cadence (5 min watch-driven + periodic), and a pointer to `docs/crd-reference.md` for status field semantics.
- [ ] T019 [P] Update `docs/crd-reference.md` to document the new `status.lastReconciledAt` field (RFC 3339; updates every reconcile; distinct from `lastScanCompletedAt`) and the v0.1 condition reasons.
- [ ] T020 Run the per-PR pre-PR gate: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && helm lint charts/mikebom-operator/` (the constitution §"Development Workflow" gate). The kind E2Es are skipped unless `MIKEBOM_OPERATOR_E2E=1` is exported.
- [ ] T021 Grep `NEEDS CLARIFICATION` across `specs/002-reconciler-skeleton/`, `crates/`, `e2e/`, and `docs/` to confirm zero leftover markers in source or spec artifacts.

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies; can start immediately.
- **Foundational (Phase 2)**: Depends on Setup. BLOCKS all user-story phases.
- **User Story 1 (Phase 3)**: Depends on Foundational. Creates `reconciler_skeleton.rs`.
- **User Story 2 (Phase 4)**: Depends on US1 (extends `reconciler_skeleton.rs`).
- **User Story 3 (Phase 5)**: T016 depends on US2 (CR exists for the Lease visibility check); T017 depends on Phase 2 only (separate file).
- **Polish (Phase 6)**: Depends on user-story phases complete.

### User Story Dependencies (graph)

```text
Setup → Foundational → US1 → US2 → US3 (T016)
                                  ↘
                                    US3 (T017, separate file, parallel-eligible)
                                  ↗
                        US3 (T017) ← Foundational
```

T017 only needs Phase 2 complete; can be drafted in parallel with US1/US2 work but should run AFTER the kind cluster has a working operator (i.e., after US1 lands).

### Within Each User Story

- **US1**: T012 alone — single self-contained authoring task.
- **US2**: T013 → T014 → T015 strictly sequential (same file, must not stomp on each other's pollers/timeouts).
- **US3**: T016 ∥ T017 — different files; both can be drafted concurrently.

### Parallel Opportunities

- **Phase 1**: T001 ∥ T002 (different files: workspace `Cargo.toml` vs chart `deployment.yaml`).
- **Phase 5**: T016 ∥ T017 (different files).
- **Phase 6**: T018 ∥ T019 (different doc files).
- **Cross-phase**: After Phase 2 lands, two developers can split US1+US2 (one file, sequential) and US3-T017 (separate file).

---

## Parallel Example: Phase 1 + early Phase 2

```bash
# Two-developer split at the start:
Task: "T001 Add tracing-subscriber json feature in workspace Cargo.toml"
Task: "T002 Add POD_NAME/POD_NAMESPACE Downward-API env vars in chart deployment.yaml"
Task: "T003 Verify RBAC verbs in chart rbac.yaml (sequential entry point for Phase 2)"
```

---

## Implementation Strategy

### MVP (Phase 1 + 2 + US1)

1. Complete Setup + Foundational → operator code compiles, unit tests pass, chart YAML in sync.
2. Complete US1 (T012) → kind E2E proves the operator runs in-cluster and emits structured logs.

At this checkpoint, the operator is **observably alive in-cluster** but doesn't yet write to any CR's status. The MVP delivers value for SRE/ops who want to install the chart and confirm "operator is healthy" without yet relying on reconciler behavior.

### Recommended full delivery (US1 + US2 + US3 + Polish)

1. Setup + Foundational → checkpoint.
2. US1 → checkpoint (MVP: chart-installed-and-runs).
3. US2 → checkpoint (reconciler acknowledges CRs — visible user-facing signal).
4. US3-T016 → checkpoint (Lease visibility verified; T017 failover deferred to opt-in).
5. Polish → checkpoint (docs + pre-PR gate).
6. PR ready.

### Parallel team split

After Phase 2:
- Developer A: US1 → US2 (sequential in same file).
- Developer B: US3-T017 (new file, only needs Phase 2 done).
- Developer C: Polish docs (after US1/US2 land).

---

## Notes

- `[P]` tasks operate on different files and have no incomplete-task dependencies.
- `[Story]` label maps each task to its user story for traceability.
- US1 is the MVP and only required deliverable for "operator runs in-cluster."
- US2 is the reconciler-functional milestone.
- US3 is multi-replica safety; T017 (the actual failover exercise) is behind a separate env-var gate because pod-kill timing is slower than the steady-state E2E.
- After each phase, commit before moving to the next so a partial PR remains coherent if work is paused.
- The chart CRD YAML regen in T005 is what keeps feature 001's drift check happy — skipping it WILL fail CI before any reconciler tests run.
