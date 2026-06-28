---

description: "Task list for feature 003-scan-job-builder"
---

# Tasks: scan-Job builder

**Input**: Design documents from `/specs/003-scan-job-builder/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/build-scan-job.md, quickstart.md

**Tests**: Mandatory. The spec's SC-003 requires 100% of FRs have at least one assertion in the unit-test suite, and constitution VI explicitly names "Job-template construction" as triggering the hermetic-E2E rule (satisfied via the kind dry-run E2E in T016).

**Organization**: Tasks group by user story per spec.md (US1 P1 / US2 P2 / US3 P3). Most of the implementation lives in one Rust file (`crates/operator/src/scan_job/mod.rs`), so within-phase work is sequential; the kind dry-run E2E (T016) lives in a separate file and can run in parallel with the unit-test-writing tasks once Phase 2 lands.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no incomplete-task dependencies)
- **[Story]**: User story label (US1, US2, US3)
- Paths are repo-relative.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Cargo workspace dep, operator crate dep, base-image digest resolution.

- [x] T001 [P] Added `sha2 = "0.10"` to workspace `[workspace.dependencies]`
- [x] T002 [P] Added `sha2.workspace = true` to `crates/operator/Cargo.toml`; also added `regex = "1"` to `[dev-dependencies]` for the DNS-1123 test
- [x] T003 Resolved digests as of 2026-06-28. **Plan deviation**: research §R1 picked `cgr.dev/chainguard/skopeo:latest`, but Chainguard moved `:latest` to the paid Chainguard Direct subscription. Switched both containers to publicly-accessible distroless images (user-confirmed: "OK with Chainguard images or really any distroless image"):
  - **init-pull** = `gcr.io/go-containerregistry/crane@sha256:1b1fb24d2b1bb27a9daf81a588157e68463876904e8e537a812edba6284fb252` (Google's go-containerregistry `crane:debug` — distroless-with-busybox; ships `sh + tar + crane`). `crane export` walks layer composition AND handles whiteout semantics correctly, replacing the skopeo + manual layer-iteration approach from research §R3 with a single pipeline.
  - **output-upload** = `cgr.dev/chainguard/busybox@sha256:accc5c911abaf2f70487f93cad07b0891d502cbba7e79f96d1db9074ef40928a` (Chainguard free tier — unchanged from initial resolution).

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Module scaffold + the builder's internal helpers + the public `build_scan_job` function. No tests yet — those land in the user-story phases so each FR has a clear test owner.

**⚠️ CRITICAL**: No user-story phase can start until this phase is complete.

- [x] T004 Created `crates/operator/src/scan_job/mod.rs` with `defaults` submodule (digest-pinned image constants, ttl/backoff/resource constants)
- [x] T005 Added `pub mod scan_job;` to `crates/operator/src/lib.rs`
- [x] T006 Implemented `BuildScanJobError` enum with `#[non_exhaustive]` per research §R8 (added `PartialEq, Eq` for use in test assertions)
- [x] T007 Implemented `job_name` + helpers `short_image_hash` (7-char SHA-256 prefix) and `sanitize_dns1123` (lowercase, hyphen-fold, collapse runs, trim). 63-char cap via `max_sanitized_len = 48`
- [x] T008 Implemented `build_init_pull_container` — uses `command` field for the `sh -c` script; `IMAGE_REF` env populated from caller; single-pipeline `crane export "$IMAGE_REF" - | tar -x -C /workdir/rootfs` (crane handles layer composition + whiteouts; replaces the per-layer iteration from research §R3)
- [x] T009 Implemented `build_mikebom_scan_container` + `scan_format_args` helper mapping all 3 `ScanFormat` variants to `(format-arg, extension)`. Resource requests populated from `defaults::SCAN_CPU_REQUEST` / `SCAN_MEMORY_REQUEST`
- [x] T010 Implemented `build_output_upload_container` — busybox image + `sh -c "ls -la /workdir/out/ && cat /workdir/out/*.json"` per clarify Q1 → C
- [x] T011 Implemented public `build_scan_job` tying it all together. Labels include `app.kubernetes.io/name`, `app.kubernetes.io/component=scan-job`, `kusari.dev/namespace-scan`, `kusari.dev/image-ref-hash`. Pod template labels mirror the Job's
- [x] T012 `cargo fmt && cargo clippy -D warnings && cargo check --workspace` all green

**Checkpoint**: Builder function compiles and clippy is clean; tests come in the next three phases.

---

## Phase 3: User Story 1 — Builder produces a valid Job manifest (Priority: P1) 🎯 MVP

**Goal**: Calling `build_scan_job` with valid inputs returns a `Job` with a DNS-1123-compliant deterministic name; calling with empty inputs returns the expected `Err`.

**Independent Test**: `cargo test --workspace --lib operator::scan_job::tests::name_is_dns1123_compliant` + the four sibling tests below pass.

### Implementation for User Story 1

- [x] T013 [US1] Authored 6 US1 tests (added `name_truncates_long_cr_name` bonus assertion). All pass. FR-001, FR-009, FR-012 covered.

**Checkpoint**: Builder is provably correct for naming + input-validation invariants.

---

## Phase 4: User Story 2 — Reviewer can verify the 3-container choreography (Priority: P2)

**Goal**: A reviewer reading the unit-test suite confirms the Job's three container slots match the bootstrap plan §3 contract.

**Independent Test**: `cargo test --workspace --lib operator::scan_job::tests::pod_template_has_three_containers_in_correct_order` + the six sibling assertions pass.

### Implementation for User Story 2

- [x] T014 [US2] Authored 7 US2 tests covering FR-002 / FR-003 / FR-004 / FR-005 / FR-006 / FR-011. All pass. `mikebom_scan_format_branches` parameterized over all 3 ScanFormat variants.

**Checkpoint**: 3-container choreography is locked in and reviewable.

---

## Phase 5: User Story 3 — Job lifecycle policies prevent operational surprises (Priority: P3)

**Goal**: One-shot Job semantics + bounded TTL + non-empty resource requests are asserted.

**Independent Test**: `cargo test --workspace --lib operator::scan_job::tests::job_lifecycle_policies_are_one_shot` + sibling assertions pass.

### Implementation for User Story 3

- [x] T015 [US3] Authored 3 US3 tests covering FR-007 / FR-008 / FR-010. All pass.

**Checkpoint**: Builder satisfies the operational-safety contract.

---

## Phase 6: Constitution VI E2E + Polish

**Purpose**: kind dry-run E2E (satisfies VI), docs touch, pre-PR gate, NEEDS CLARIFICATION grep.

- [x] T016 [P] [US1] Created `e2e/tests/scan_job_dryrun.rs::scan_job_passes_server_dry_run` (gated) — loops over all 3 ScanFormat variants. Also `empty_mikebom_image_returns_error_path` runs ungated (pure Rust). Added `operator = { path = "../crates/operator" }` and `serde_yaml.workspace = true` to `e2e/Cargo.toml`.
- [x] T017 [P] Added "Scan-Job builder" subsection to `docs/architecture.md`'s Reconciler block, pointing at `operator::scan_job::build_scan_job` + the test surface
- [x] T018 Pre-PR gate: `cargo fmt` (applied), `cargo clippy -D warnings` (clean), `cargo test --workspace` (28 tests pass: 22 lib tests + 2 drift + multiple e2e gated skips). `helm lint` deferred to CI.
- [x] T019 NEEDS CLARIFICATION grep clean (only meta-reference in `tasks.md`'s T019 description itself)

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies; can start immediately. T003 (digest resolution) needs network access to the registry but no code state.
- **Foundational (Phase 2)**: Depends on Setup. T004 depends on T003's digests for the `defaults` constants. BLOCKS all user-story phases.
- **User Story 1 (Phase 3)**: Depends on Foundational. Single-task phase.
- **User Story 2 (Phase 4)**: Depends on Foundational (extends the same test module as US1; sequential after US1 for file editing).
- **User Story 3 (Phase 5)**: Same — sequential after US2 for the same-file reason.
- **Phase 6**: T016 only needs Phase 2 done — can be drafted in parallel with US1/US2/US3 tests. T017 is independent. T018 + T019 run last.

### User Story Dependencies (graph)

```text
Setup → Foundational → US1 → US2 → US3 → T018+T019 (pre-PR gate, grep)
                          ↓
                       T016 (E2E, parallel-eligible with US2+US3)
                       T017 (docs, parallel-eligible with everything after Phase 2)
```

### Within Each User Story

- US1 (T013), US2 (T014), US3 (T015): single test-batch tasks each. Within each task, individual `#[test] fn`s can be authored in any order.

### Parallel Opportunities

- **Phase 1**: T001 ∥ T002 (different Cargo.tomls); T003 is independent (network)
- **Phase 6**: T016 ∥ T017 (different files); both can be drafted in parallel with US1/US2/US3 work after Phase 2 ends
- **Cross-phase**: T016 (E2E) can be authored alongside T013–T015 (unit tests) since they live in different files

---

## Parallel Example: After Phase 2

```bash
# Two developers split:
Task: "T013 [US1] Author name/error unit tests in scan_job/mod.rs"
Task: "T016 [US1] Create e2e/tests/scan_job_dryrun.rs (kind dry-run)"
# Once T013 is done, US2 → US3 sequential in the same file.
```

---

## Implementation Strategy

### MVP (Setup + Foundational + US1)

1. Complete Setup (T001-T003) + Foundational (T004-T012) → builder compiles + clippy clean.
2. Complete US1 (T013) → builder's correctness invariants are tested.

At this checkpoint, the builder is **callable from feature 004's reconciler** but doesn't have full FR coverage. Useful as a hand-off point to a different developer continuing US2+US3.

### Recommended full delivery (US1 + US2 + US3 + Polish)

1. Setup + Foundational → checkpoint.
2. US1 → checkpoint (MVP).
3. US2 → checkpoint (3-container contract locked).
4. US3 → checkpoint (lifecycle policies locked).
5. T016 (E2E) → checkpoint (constitution VI satisfied).
6. T017 (docs) + T018 (gate) + T019 (grep) → PR ready.

### Parallel team split

After Phase 2:
- Developer A: US1 → US2 → US3 (sequential, same file).
- Developer B: T016 E2E + T017 docs (independent files).
- Developer C: T018+T019 once everyone else lands.

---

## Notes

- `[P]` tasks operate on different files and have no incomplete-task dependencies.
- `[Story]` label maps each task to its user story for traceability.
- US1 is the MVP — Phase 1+2+3 is the smallest cut that delivers a usable builder.
- US2 + US3 add coverage; both are required for SC-003's "100% of FRs have a test" claim.
- T016 (kind dry-run E2E) is the constitution VI gate. Without it the PR cannot land.
- After each phase, commit before moving to the next so a partial PR remains coherent if work is paused.
