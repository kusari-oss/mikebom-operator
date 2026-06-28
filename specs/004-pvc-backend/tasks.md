---

description: "Task list for feature 004-pvc-backend"
---

# Tasks: PVC output backend

**Input**: Design documents from `/specs/004-pvc-backend/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/output-backends.md, quickstart.md

**Tests**: Mandatory. The spec's SC-003 requires 100% of FRs have at least one unit test; constitution VI requires a kind-based E2E for Job-template changes (satisfied via the PVC variant added to `scan_job_dryrun.rs`).

**Organization**: Tasks group by user story per spec.md (US1 P1 / US2 P2 / US3 P3). Most work lands in one file (`crates/operator/src/scan_job/mod.rs`), so within-phase work is sequential; the e2e test (T012) and chart/docs (T013, T014) can run in parallel with each other after Phase 2.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no incomplete-task dependencies)
- **[Story]**: User story label (US1, US2, US3)
- Paths are repo-relative.

---

## Phase 1: Setup (Sanity Check)

**Purpose**: Confirm the inherited test surface from feature 003 is healthy before touching it. No new deps; no chart RBAC changes; no CRD changes.

- [x] T001 Baseline confirmed via prior session: 22 lib tests passing (16 scan_job + 6 status)

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: All the dispatch infrastructure + the PVC helpers + the public surface change. No user-story tests yet — they layer on top in Phase 3+.

**⚠️ CRITICAL**: No user-story phase can start until this phase is complete.

- [x] T002 Added `MissingPvcConfig` variant to `BuildScanJobError` enum
- [x] T003 Added `PVC_VOLUME_NAME` + `PVC_MOUNT_PATH` constants to `defaults`
- [x] T004 Implemented `pvc_output_volume(claim_name)` helper
- [x] T005 Implemented `pvc_output_mount()` helper
- [x] T006 Implemented `strip_leading_slash(s)` helper
- [x] T007 Implemented `build_output_upload_pvc_container(pvc)` — uses `set -eu`, `${PATH_PREFIX:+/${PATH_PREFIX}}` shell expansion, env-var-based path injection (no caller-controlled metacharacters reach the command string)
- [x] T008 Renamed `build_output_upload_container` → `build_output_upload_placeholder_container`
- [x] T009a Added dispatch fn `build_output_upload_container(output) -> Result<Container, BuildScanJobError>` — Pvc validates + delegates; S3/Oci return the placeholder
- [x] T009b Updated `valid_spec()` to `OutputType::S3` with `pvc/s3/oci: None` so the 22 inherited tests continue to exercise the placeholder branch
- [x] T010 `build_scan_job` now pre-computes `output_upload = build_output_upload_container(&spec.output)?` and the `volumes` vec (conditionally pushing `pvc_output_volume` when output.type=Pvc with non-empty claim_name)
- [x] T011 `cargo check + fmt + clippy -D warnings` all green

**Checkpoint**: Dispatch is wired. The non-PVC arm produces the same output as feature 003 did (verified by feature 003's existing tests in Phase 5); the PVC arm needs explicit tests, which come in Phase 3.

---

## Phase 3: User Story 1 — Builder produces a PVC-backed Job (Priority: P1) 🎯 MVP

**Goal**: With `spec.output.type=Pvc` and a non-empty `claim_name`, `build_scan_job` returns a Job whose pod template has the PVC volume + mount on `output-upload` only, and whose command copies SBOMs to the configured destination.

**Independent Test**: `cargo test --workspace --lib operator::scan_job::tests::pvc_dispatch_adds_pvc_volume_to_pod_spec` + sibling assertions pass.

### Implementation for User Story 1

- [x] T012 [US1] Added `valid_pvc_spec(claim_name, path_prefix)` helper + 8 unit tests (all passing): `pvc_dispatch_adds_pvc_volume_to_pod_spec`, `pvc_output_upload_mounts_pvc_at_known_path`, `pvc_volume_mounted_only_on_output_upload`, `pvc_output_upload_copies_to_pvc_mount`, `pvc_output_upload_respects_path_prefix`, `path_prefix_strips_leading_slash`, `missing_pvc_config_errors_when_pvc_none`, `missing_pvc_config_errors_when_claim_empty`
- [x] T013 [US1] Added `pvc_scan_job_passes_server_dry_run` to `e2e/tests/scan_job_dryrun.rs` (gated by `MIKEBOM_OPERATOR_E2E=1`) — loops over all 3 ScanFormat variants with `valid_pvc_spec("sbom-scratch", "team-a")`

**Checkpoint**: PVC dispatch is provably correct. Builder ready for feature 005+ to layer S3/OCI dispatches.

---

## Phase 4: User Story 2 — Cluster admin can configure the chart for PVC output (Priority: P2)

**Goal**: A cluster admin reading the chart's `values.yaml` and `docs/crd-reference.md` finds an end-to-end worked example of `output.type=pvc`.

**Independent Test**: Manual review of the merged docs; no automated assertion (acceptable per spec FR-010).

### Implementation for User Story 2

- [x] T014 [P] [US2] Added commented `output:` block to `charts/mikebom-operator/values.yaml` showing the PVC backend wiring + operator-doesn't-create-PVC note
- [x] T015 [P] [US2] Added "Output backends" section to `docs/crd-reference.md` with PVC example YAML, PVC provisioning snippet, access-mode (RWO/RWX) table, and S3/OCI placeholder note

**Checkpoint**: Documentation surface for PVC backend exists and is reviewable.

---

## Phase 5: User Story 3 — Existing builder tests stay green (Priority: P3)

**Goal**: feature 001/002/003 tests + the renamed feature 003 placeholder test all pass after feature 004's dispatch refactor.

**Independent Test**: `cargo test --workspace` shows zero failures across the entire suite, including the renamed `output_upload_non_pvc_is_v03_placeholder` test.

### Implementation for User Story 3

- [x] T016 [US3] Renamed `output_upload_is_v03_placeholder` → `output_upload_non_pvc_is_v03_placeholder`. The valid_spec() update in T009b made this rename semantically correct (now exercises the OutputType::S3 placeholder branch via the shared fixture)
- [x] T017 [US3] `cargo test --workspace` green: 30 lib (24 scan_job — 16 feat-003 + 8 new feat-004 — + 6 status), 2 drift, 4 e2e gated skips. Total 36 test runs pass

**Checkpoint**: Regression coverage intact. v0.4 ships without breaking any prior contract.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Pre-PR gate + NEEDS CLARIFICATION grep.

- [x] T018 Pre-PR gate green: cargo fmt --check, cargo clippy -D warnings, cargo test --workspace (30 lib + 2 drift + 4 e2e gated). helm lint deferred to CI (SHA-pinned azure/setup-helm@v4)
- [x] T019 NEEDS CLARIFICATION grep clean (only meta-reference in T019's own description text)

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies; can start immediately. T001 is a baseline check.
- **Foundational (Phase 2)**: Depends on Setup. BLOCKS all user-story phases. T002–T010 are largely sequential within `scan_job/mod.rs` (same file edits). T009a + T009b must land in the same commit so the intermediate state (dispatch wired, fixtures broken) doesn't ship.
- **User Story 1 (Phase 3)**: Depends on Foundational. T012 (unit tests) and T013 (E2E) can be drafted in parallel.
- **User Story 2 (Phase 4)**: Depends on Foundational only (chart + docs are independent files). T014 ∥ T015.
- **User Story 3 (Phase 5)**: Depends on Foundational (the rename + regression run only make sense after the dispatch lands). T016 is single-file; T017 is the run.
- **Polish (Phase 6)**: Depends on user-story phases complete.

### User Story Dependencies (graph)

```text
Setup → Foundational ─┬─→ US1 (T012 ∥ T013)
                      ├─→ US2 (T014 ∥ T015) — parallel-eligible across stories
                      └─→ US3 (T016 → T017)
                              └─→ Polish (T018 → T019)
```

US1, US2, US3 can all start as soon as Foundational completes; they touch different files and don't conflict.

### Parallel Opportunities

- **Phase 4**: T014 ∥ T015 (chart vs docs)
- **Phase 3 + 4 + 5**: After Foundational, three developers can take US1, US2, US3 concurrently
- **Within US1**: T012 (unit tests) ∥ T013 (E2E) — different files

---

## Parallel Example: After Phase 2

```bash
# Three-developer split once dispatch is wired:
Task: "T012 [US1] Author 8 unit tests covering FR-001/002/003/005/007/008/009 in scan_job/mod.rs::tests"
Task: "T013 [US1] Add pvc_scan_job_passes_server_dry_run to e2e/tests/scan_job_dryrun.rs"
Task: "T014 [US2] Add commented mikebom.output block to charts/mikebom-operator/values.yaml"
Task: "T015 [US2] Add Output backends section to docs/crd-reference.md"
```

---

## Implementation Strategy

### MVP (Setup + Foundational + US1)

1. Complete T001 → confirm baseline.
2. Complete T002–T011 → dispatch wired, compiles, clippy clean.
3. Complete T012–T013 → PVC dispatch tested + manifest validated against a kind API server.

At this checkpoint, the builder is **fully usable** by a future reconciler integration; cluster docs are missing but won't block the feature 005 / 006 contributors.

### Recommended full delivery

1. Setup + Foundational → checkpoint.
2. US1 → checkpoint (MVP).
3. US2 → checkpoint (docs).
4. US3 → checkpoint (regression coverage explicit).
5. Polish → checkpoint (pre-PR gate + grep).
6. PR ready.

### Parallel team split

After Phase 2:
- Developer A: US1 (T012 → T013).
- Developer B: US2 (T014 ∥ T015 in different files).
- Developer C: US3 (T016 → T017) then Polish (T018 → T019).

---

## Notes

- `[P]` tasks operate on different files and have no incomplete-task dependencies.
- `[Story]` label maps each task to its user story.
- US1 is the MVP — Phase 1+2+3 ships the working dispatch.
- US2 ships the documentation; without it the feature is invisible to cluster admins.
- US3 is regression coverage; without it nothing forces the rename in T016 to happen.
- After each phase, commit so a partial PR remains coherent if work is paused.
- T016's `valid_spec()` semantics shift is the trickiest task — read carefully; the renamed test must exercise the non-PVC arm, which means the fixture must NOT have `output.type=Pvc` (or must override it locally).
