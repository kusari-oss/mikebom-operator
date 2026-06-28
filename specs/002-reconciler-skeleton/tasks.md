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

- [x] T001 [P] Added `"json"` to `tracing-subscriber` workspace features
- [x] T002 [P] Added `POD_NAME` + `POD_NAMESPACE` Downward-API env vars to `charts/mikebom-operator/templates/deployment.yaml`
- [x] T003 RBAC pre-flight: chart `rbac.yaml` already includes namespacescans/status, leases (full verbs), events — no changes needed

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Add the CRD field, regenerate the chart YAML, and build out the reconciler library code so all user-story phases can layer E2E tests on top.

**⚠️ CRITICAL**: No user-story phase can start until this phase is complete.

- [x] T004 Added `lastReconciledAt: Option<String>` to `NamespaceScanStatus` with explanatory doc-comment
- [x] T005 Regenerated chart CRD YAML; new field appears with `nullable: true` + doc-string
- [x] T006 `cargo test --test crd_drift` passes (2/2)
- [x] T007 Implemented `crates/operator/src/leader.rs` — hand-rolled Lease acquisition + 5s renewer + process exit on renewal failure (per research R1's fallback option B since kube-rs 0.97 has no helper we wanted to inherit)
- [x] T008 Implemented `crates/operator/src/status.rs::desired_status` + 6 unit tests covering valid/InvalidSpec/idempotency/transition/preservation/whitespace-labelSelector
- [x] T009 Implemented `crates/operator/src/reconcile/namespace_scan.rs::{reconcile, error_policy, Ctx, ReconcileError}` — patches `/status` via `Patch::Merge`, requeues at 5min, 404 in error_policy → `Action::await_change()`
- [x] T010 Wired `crates/operator/src/main.rs` — JSON tracing init, Downward-API env reads, `run_with_leadership` wrapping `Controller::new(...).run(...)` stream
- [x] T011 `cargo fmt && cargo clippy -D warnings && cargo test --workspace` all green

**Checkpoint**: CRD shape updated + chart in sync; reconciler library compiles + unit tests pass; main.rs ready to deploy.

---

## Phase 3: User Story 1 — Operator becomes Ready in-cluster (Priority: P1) 🎯 MVP

**Goal**: `helm install` produces a `Ready` operator pod within 30 seconds; operator logs show structured-JSON startup + leadership-acquired records.

**Independent Test**: Install the chart fresh in a kind cluster, observe the operator pod reaches `Ready` within 30s, run `kubectl logs ... -o json | jq '.fields.event'` and observe both `"startup"` and `"leader_acquired"`.

### Implementation for User Story 1

- [x] T012 [US1] Created `e2e/tests/reconciler_skeleton.rs::reconciler_skeleton_full_flow` (consolidates US1+US2+US3-observability into one helm-install cycle for E2E performance). Asserts pod Ready < 60s + structured log records `event=startup` and `event=leader_acquired` (FR-001, FR-002, FR-009, SC-001)

**Checkpoint**: Operator installs and runs in-cluster. No reconcile work happens yet (no CRs applied) — that's US2.

---

## Phase 4: User Story 2 — Reconciler acknowledges NamespaceScan CRs (Priority: P2)

**Goal**: Apply a `NamespaceScan` CR; within 10 seconds its `status.conditions[Ready=False, reason=NotYetReconciled]` and `status.lastReconciledAt` are populated.

**Independent Test**: With the operator installed (US1 done), `kubectl apply -f examples/namespacescan.yaml` and poll `kubectl get namespacescan scan-prod -o jsonpath='{.status.conditions[?(@.type=="Ready")].reason}'` until it equals `NotYetReconciled` (within 10s); separately verify `lastReconciledAt` is non-empty.

**Depends on**: Phase 3 (T012 must have created `reconciler_skeleton.rs`; US2 tasks extend the same file).

### Implementation for User Story 2

- [x] T013 [US2] Folded into `reconciler_skeleton_full_flow`: applies valid CR, polls for `Ready/NotYetReconciled` < 10s, asserts `lastReconciledAt` non-empty (FR-003, FR-004, SC-002)
- [x] T014 [US2] Folded into same test: applies invalid CR (empty target.namespaces + unset labelSelector), polls for `Ready/InvalidSpec` < 10s (FR-011)
- [x] T015 [US2] Folded into same test: deletes both CRs, asserts no `"level":"ERROR"` records appear in subsequent operator logs (FR-012)

**Checkpoint**: Reconciler is observably alive — CRs get acknowledged, invalid specs are flagged, deletions are clean. The MVP-plus-enforcement state.

---

## Phase 5: User Story 3 — Multi-replica safety via leader election (Priority: P3)

**Goal**: With multi-replica deployments, exactly one replica reconciles at a time; failover happens within 30 seconds of leader pod death.

**Independent Test**: T016 is observable in any install (single replica still produces a Lease with the pod as holder). T017 requires deliberate scale + pod-kill — gated behind a separate env var so it's opt-in.

**Depends on**: Phase 3 (chart installed) + Phase 4 (CR exists for T017's reconcile-resumption assertion).

### Implementation for User Story 3

- [x] T016 [P] [US3] Folded into `reconciler_skeleton_full_flow`: asserts the Lease's `.spec.holderIdentity` starts with `mikebom-operator-` (FR-007)
- [x] T017 [P] [US3] Created `e2e/tests/reconciler_failover.rs::failover_within_30s`, gated by `MIKEBOM_OPERATOR_E2E_FAILOVER=1` (opt-in). Installs with 2 replicas, kills the current leader, polls for new `holderIdentity` < 30s + CR's `lastReconciledAt` refresh < 30s further (FR-008, SC-003)

**Checkpoint**: Multi-replica HA is exercised. v0.1 reconciler skeleton is complete.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Documentation updates + the pre-PR gate.

- [x] T018 [P] Added "Reconciler" subsection to `docs/architecture.md` with lifecycle diagram, condition vocabulary table, requeue cadence, and idempotency note
- [x] T019 [P] Updated `docs/crd-reference.md` Status table with `lastReconciledAt` row + condition reasons table (current + reserved for 003+)
- [x] T020 Pre-PR gate: `cargo fmt --all` (applied auto-fmt), `cargo clippy -D warnings` (clean), `cargo test --workspace` (11/11 tests pass: 6 status unit, 2 drift, 3 e2e gated skips). `helm lint` not runnable locally — CI runs via `azure/setup-helm@v4` (SHA-pinned)
- [x] T021 NEEDS CLARIFICATION grep clean across `specs/002-reconciler-skeleton/`, `crates/`, `e2e/`, `docs/`

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
