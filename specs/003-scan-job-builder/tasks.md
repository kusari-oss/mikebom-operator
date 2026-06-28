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

- [ ] T001 [P] Add `sha2 = "0.10"` to `[workspace.dependencies]` in workspace `Cargo.toml`
- [ ] T002 [P] Add `sha2.workspace = true` to `[dependencies]` in `crates/operator/Cargo.toml`
- [ ] T003 Resolve manifest-list digests for the two base images via `crane digest cgr.dev/chainguard/skopeo:latest` and `crane digest cgr.dev/chainguard/busybox:latest` (matches feature 001's digest-pinning convention). Record both digests + the resolution date for the inline `# latest as of YYYY-MM-DD` comment.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Module scaffold + the builder's internal helpers + the public `build_scan_job` function. No tests yet — those land in the user-story phases so each FR has a clear test owner.

**⚠️ CRITICAL**: No user-story phase can start until this phase is complete.

- [ ] T004 Create `crates/operator/src/scan_job/mod.rs` with the module skeleton: `pub use` statements, `defaults` submodule containing `INIT_PULL_IMAGE`, `OUTPUT_UPLOAD_IMAGE` (digest values from T003), `TTL_SECONDS_AFTER_FINISHED = 3600`, `BACKOFF_LIMIT = 2`, `SCAN_CPU_REQUEST = "100m"`, `SCAN_MEMORY_REQUEST = "128Mi"` constants
- [ ] T005 Add `pub mod scan_job;` to `crates/operator/src/lib.rs` so `operator::scan_job::build_scan_job` is the public path
- [ ] T006 In `crates/operator/src/scan_job/mod.rs`, implement `pub enum BuildScanJobError` with `EmptyMikebomImage` and `EmptyImageRef` variants via `thiserror::Error` per research §R8
- [ ] T007 Implement `fn job_name(cr_name: &str, image_ref: &str) -> String` helper: SHA-256 of `image_ref`, take first 7 hex chars, sanitize `cr_name` to DNS-1123, format as `nsscan-<sanitized>-<hash>`, truncate the sanitized portion if total length would exceed 63 chars (research §R5)
- [ ] T008 Implement `fn build_init_pull_container(image_ref: &str) -> Container` helper: image = `defaults::INIT_PULL_IMAGE`, env carries `IMAGE_REF`, args = `["sh", "-c", "skopeo copy ... && tar -x ..."]` per research §R3, volumeMount on `workdir` at `/workdir`
- [ ] T009 Implement `fn build_mikebom_scan_container(spec: &NamespaceScanSpec, short_hash: &str) -> Container` helper: image = `spec.mikebom_image`, args invoke `mikebom sbom scan --path /workdir/rootfs --format <format> --output <format>=/workdir/out/<short_hash>.<ext>` where `<format>` maps from `spec.scan_format` (CyclonedxJson → `cyclonedx-json` + `.cdx.json`; Spdx23Json → `spdx-2.3-json` + `.spdx.json`; Spdx3Json → `spdx-3-json` + `.spdx3.json`), `resources.requests` populated with `defaults::SCAN_CPU_REQUEST` / `SCAN_MEMORY_REQUEST`, volumeMount on `workdir`
- [ ] T010 Implement `fn build_output_upload_container() -> Container` helper: image = `defaults::OUTPUT_UPLOAD_IMAGE`, args = `["sh", "-c", "ls -la /workdir/out/ && cat /workdir/out/*.json"]`, volumeMount on `workdir`
- [ ] T011 Implement the public `pub fn build_scan_job(spec: &NamespaceScanSpec, cr_name: &str, image_ref: &str) -> Result<batch::v1::Job, BuildScanJobError>` tying it all together: validate inputs (empty checks → return `Err`), compute name + short hash, populate `Job::default()`'s metadata.name + labels (per data-model §2), `Job::spec` with `completions=1`, `parallelism=1`, `backoff_limit=2`, `ttl_seconds_after_finished=3600`, `template.spec` with `restart_policy="Never"`, `volumes=[workdir emptyDir]`, `init_containers=[init-pull, mikebom-scan]`, `containers=[output-upload]`
- [ ] T012 Run `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo check --workspace` and resolve before moving on

**Checkpoint**: Builder function compiles and clippy is clean; tests come in the next three phases.

---

## Phase 3: User Story 1 — Builder produces a valid Job manifest (Priority: P1) 🎯 MVP

**Goal**: Calling `build_scan_job` with valid inputs returns a `Job` with a DNS-1123-compliant deterministic name; calling with empty inputs returns the expected `Err`.

**Independent Test**: `cargo test --workspace --lib operator::scan_job::tests::name_is_dns1123_compliant` + the four sibling tests below pass.

### Implementation for User Story 1

- [ ] T013 [US1] Add `#[cfg(test)] mod tests` to `crates/operator/src/scan_job/mod.rs` with helpers (`valid_spec()`, etc.) and unit tests covering FR-001, FR-009, FR-012:
  - `name_is_dns1123_compliant` — regex assertion that name matches `^[a-z0-9]([-a-z0-9]*[a-z0-9])?$` and `name.len() <= 63`
  - `name_is_deterministic` — two calls with identical `(cr_name, image_ref)` produce the same name
  - `name_differs_for_different_images` — calls varying `image_ref` produce distinct names
  - `empty_mikebom_image_errors` — `spec.mikebom_image = ""` returns `Err(BuildScanJobError::EmptyMikebomImage)`
  - `empty_image_ref_errors` — `image_ref = ""` returns `Err(BuildScanJobError::EmptyImageRef)`

**Checkpoint**: Builder is provably correct for naming + input-validation invariants.

---

## Phase 4: User Story 2 — Reviewer can verify the 3-container choreography (Priority: P2)

**Goal**: A reviewer reading the unit-test suite confirms the Job's three container slots match the bootstrap plan §3 contract.

**Independent Test**: `cargo test --workspace --lib operator::scan_job::tests::pod_template_has_three_containers_in_correct_order` + the six sibling assertions pass.

### Implementation for User Story 2

- [ ] T014 [US2] Extend `#[cfg(test)] mod tests` with assertions covering FR-002, FR-003, FR-004, FR-005, FR-006, FR-011:
  - `pod_template_has_three_containers_in_correct_order` — `init_containers[0].name == "init-pull"`, `init_containers[1].name == "mikebom-scan"`, `containers[0].name == "output-upload"`
  - `all_containers_share_workdir_emptydir` — single `Volume { name: "workdir", emptyDir: Some(_), .. }` in pod spec; all three container slots mount it at `/workdir`
  - `init_pull_extracts_rootfs` — `init-pull.image == defaults::INIT_PULL_IMAGE`; args contain `"skopeo copy"`; env contains `IMAGE_REF`
  - `mikebom_scan_uses_spec_image_and_args` — `mikebom-scan.image == spec.mikebom_image`; args include `["sbom", "scan", "--path", "/workdir/rootfs"]`
  - `mikebom_scan_format_branches` — parameterized over the three `ScanFormat` variants; asserts `--format cyclonedx-json` ↔ `.cdx.json`, `--format spdx-2.3-json` ↔ `.spdx.json`, `--format spdx-3-json` ↔ `.spdx3.json`
  - `output_upload_is_v03_placeholder` — `output-upload.image == defaults::OUTPUT_UPLOAD_IMAGE`; args contain `"ls -la /workdir/out/"` and `"cat /workdir/out/*.json"`
  - `all_container_images_are_pinned` — for each of init-pull, mikebom-scan, output-upload: image string contains `"@sha256:"` OR a tag suffix other than `:latest`

**Checkpoint**: 3-container choreography is locked in and reviewable.

---

## Phase 5: User Story 3 — Job lifecycle policies prevent operational surprises (Priority: P3)

**Goal**: One-shot Job semantics + bounded TTL + non-empty resource requests are asserted.

**Independent Test**: `cargo test --workspace --lib operator::scan_job::tests::job_lifecycle_policies_are_one_shot` + sibling assertions pass.

### Implementation for User Story 3

- [ ] T015 [US3] Extend `#[cfg(test)] mod tests` with assertions covering FR-007, FR-008, FR-010:
  - `job_lifecycle_policies_are_one_shot` — `spec.completions == Some(1)`, `spec.parallelism == Some(1)`, `spec.backoff_limit <= Some(3)`, `template.spec.restart_policy == Some("Never".to_string())`
  - `ttl_within_one_hour` — `spec.ttl_seconds_after_finished` is `Some(v)` with `0 < v <= 3600`
  - `mikebom_scan_has_resource_requests` — `mikebom-scan.resources.as_ref().and_then(|r| r.requests.as_ref()).map(|r| !r.is_empty()) == Some(true)`; both `cpu` and `memory` keys present

**Checkpoint**: Builder satisfies the operational-safety contract.

---

## Phase 6: Constitution VI E2E + Polish

**Purpose**: kind dry-run E2E (satisfies VI), docs touch, pre-PR gate, NEEDS CLARIFICATION grep.

- [ ] T016 [P] [US1] Create `e2e/tests/scan_job_dryrun.rs` (gated by `MIKEBOM_OPERATOR_E2E=1`) with `scan_job_passes_server_dry_run` test: constructs fixture `NamespaceScanSpec`, calls `build_scan_job(&spec, "scan-prod", "nginx:1.27.0")?`, serializes to YAML via `serde_yaml::to_string`, pipes through `kubectl apply --dry-run=server -f - --kube-context kind-mikebom-operator-e2e -n default`, asserts success. Includes sub-tests for `ScanFormat::Spdx3Json` and for the empty-image error path. Per research §R7. Can be drafted in parallel with T013-T015 once Phase 2 lands; labeled US1 because manifest validity is the P1 story.
- [ ] T017 [P] Add a sentence to `docs/architecture.md`'s Reconciler subsection pointing at the new `operator::scan_job::build_scan_job` function as the canonical Job-spec entry point that feature 004+ will call
- [ ] T018 Run the per-PR gate: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && helm lint charts/mikebom-operator/` (helm lint runs in CI if not on local PATH)
- [ ] T019 Grep `NEEDS CLARIFICATION` across `specs/003-scan-job-builder/`, `crates/operator/src/scan_job/`, `e2e/tests/`, and `docs/` to confirm zero leftover markers

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
