---

description: "Task list for feature 001-crd-yaml-generator"
---

# Tasks: CRD YAML generator + drift check

**Input**: Design documents from `/specs/001-crd-yaml-generator/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/cli.md, quickstart.md

**Tests**: The spec explicitly mandates test artifacts (FR-005 + FR-007 require an integration test; FR-004 + R2 require a determinism sub-test; constitution VI requires a kind-based E2E for CRD-shape changes). Test tasks here are **deliverables**, not optional.

**Organization**: Tasks are grouped by user story. Each phase is independently testable; phases run sequentially (US1 → US2 → US3) because US2's drift test depends on US1's regen command having populated the chart YAML in T011.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (US1, US2, US3)
- All paths are repository-relative.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Cargo dependency additions that all later phases need.

- [x] T001 [P] Add `pretty_assertions = "1"` to `[dev-dependencies]` in `crates/operator/Cargo.toml` (also added `serde_yaml.workspace = true` to runtime deps — required by `serialize.rs`)
- [x] T002 [P] Add `operator = { path = "../operator" }` to `[dependencies]` in `crates/ctl/Cargo.toml`

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Convert the operator crate from bin-only to lib + bin so the `ctl` binary and the operator's integration test can share one serializer.

**⚠️ CRITICAL**: No user story work can begin until this phase is complete.

- [x] T003 [P] Create `crates/operator/src/lib.rs` declaring `pub mod crds; pub mod output; pub mod reconcile; pub mod status;`
- [x] T004 [P] Update `crates/operator/src/main.rs` to drop the inline `mod crds; mod output; mod reconcile; mod status;` lines (the modules are now reachable through `operator::…` via the new lib)
- [x] T005 Add `pub mod serialize;` to `crates/operator/src/crds/mod.rs` (depends on T003)
- [x] T006 Create `crates/operator/src/crds/serialize.rs` exposing `pub fn crd_yaml<K: kube::CustomResourceExt>() -> String` that calls `serde_yaml::to_string(&K::crd()).expect("CRD serialization is infallible")` (depends on T005)
- [x] T007 Run `cargo check --workspace` and confirm the workspace builds with the new lib + bin layout (depends on T001, T002, T003, T004, T005, T006)

**Checkpoint**: Operator is lib + bin; serializer exists; ctl can `use operator::…`.

---

## Phase 3: User Story 1 — Regenerate CRD YAML from Rust source (Priority: P1) 🎯 MVP

**Goal**: A contributor can run a single command to emit the `NamespaceScan` CRD as YAML and overwrite the chart's CRD file.

**Independent Test**: Run `cargo run --bin mikebom-operator-ctl -- crd` and visually confirm the stdout is a valid `apiextensions.k8s.io/v1` `CustomResourceDefinition` matching the `NamespaceScan` Rust struct. Run with `--output PATH` and confirm the file is written.

### Implementation for User Story 1

- [x] T008 [US1] Rewrite `crates/ctl/src/main.rs` to define a `clap::Subcommand` enum with a `Crd { output: Option<PathBuf> }` variant and dispatch to `operator::crds::serialize::crd_yaml::<operator::crds::namespace_scan::NamespaceScan>()`
- [x] T009 [US1] In `crates/ctl/src/main.rs`, implement `--output` handling: when `Some(path)`, write the generated string to `path` (overwrite) and exit 0; when `None`, print to stdout
- [x] T010 [US1] Ran `cargo run --bin mikebom-operator-ctl -- crd` and verified output via structural grep (per analyzer A1 recommendation): `apiVersion: apiextensions.k8s.io/v1`, `kind: CustomResourceDefinition`, `metadata.name: namespacescans.kusari.dev`, `spec.group: kusari.dev` all present
- [x] T011 [US1] Ran `cargo run --bin mikebom-operator-ctl -- crd --output charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml` — chart YAML now 167 lines of real generated content (FR-009 satisfied)
- [x] T012 [US1] Added "Regenerating the chart CRD YAML" section to `docs/crd-reference.md`

**Checkpoint**: Regen command works; chart YAML now contains real generated CRD; chart is theoretically installable.

---

## Phase 4: User Story 2 — CI rejects drifted PRs (Priority: P2)

**Goal**: A `cargo test --workspace` run fails the build if the chart's CRD YAML diverges from what the generator produces from the current Rust struct.

**Independent Test**: Locally edit a field in `crates/operator/src/crds/namespace_scan.rs`. Run `cargo test --workspace`. Observe `crd_drift::chart_crd_yaml_matches_generator` fail with a diagnostic that names the regen command verbatim.

**Depends on**: Phase 3 (T011 must have populated the chart YAML with real content, or the drift test would fail on its very first run).

### Implementation for User Story 2

- [x] T013 [US2] Created `crates/operator/tests/crd_drift.rs::chart_crd_yaml_matches_generator` with `include_str!` of chart YAML and `pretty_assertions::assert_str_eq!`; failure message embeds the regen command verbatim (FR-006)
- [x] T014 [US2] Added `generator_is_deterministic` in the same file (two-call byte equality; FR-004 / research §R2)
- [x] T015 [US2] `cargo test --test crd_drift` — both tests pass
- [x] T016 [US2] Drift simulation: changed `scope: Namespaced` → `scope: Cluster` in chart YAML; `cargo test` failed with verbatim regen command in diagnostic; reverted; tests pass again

**Checkpoint**: Drift check is live. CI on this branch will now block any PR that introduces struct/YAML drift.

---

## Phase 5: User Story 3 — Chart installs the CRD (Priority: P3)

**Goal**: Chart consumers can `helm install` the chart and immediately apply `NamespaceScan` CRs because the chart ships a real CRD (not a placeholder).

**Independent Test**: In a fresh kind cluster, `helm install mikebom-operator charts/mikebom-operator -n kusari-operator --create-namespace` succeeds and `kubectl get crd namespacescans.kusari.dev` returns a valid CRD.

**Depends on**: Phase 3 (chart YAML must be real, per T011).

### Implementation for User Story 3

- [x] T017 [US3] Created `e2e/tests/crd_install.rs::helm_install_registers_crd` — gated, shells out to `helm install` (60s `--wait`) and `kubectl get crd namespacescans.kusari.dev`, cleans up
- [x] T018 [US3] Added "Testing" subsection to `docs/architecture.md` (unit/integration + kind-E2E with invocation example)

**Checkpoint**: All three user stories are independently functional. The chart is installable, the drift check is enforced, and the regen command is the documented contributor workflow.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Documentation refresh and pre-PR gate validation.

- [x] T019 [P] Added regen workflow + drift-check explanation to `docs/README.md` (new "Editing the `NamespaceScan` CRD" section)
- [x] T020 [P] Added "Source of truth for CRDs" subsection to `docs/architecture.md` with the Rust→serializer→chart pipeline diagram
- [x] T021 Pre-PR gate: `cargo fmt --all` (applied — bootstrap `crd_install.rs`/`crd_drift.rs` reflowed), `cargo clippy --workspace --all-targets -- -D warnings` (clean — fixed `needless_return` in bootstrap `namespace_scan_baseline.rs`), `cargo test --workspace` (4/4 green). `helm lint` deferred to CI (`helm` not on local PATH; `azure/setup-helm@v4` runs it in `.github/workflows/ci.yml`).
- [x] T022 Grep clean: 3 hits, all meta-references (this task line + 2 checklist items in `requirements.md`); zero actual `NEEDS CLARIFICATION` markers in spec.md / plan.md / research.md / data-model.md / contracts/ / source code

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies; can start immediately.
- **Foundational (Phase 2)**: Depends on Setup; BLOCKS all user-story phases.
- **User Story 1 (Phase 3)**: Depends on Foundational.
- **User Story 2 (Phase 4)**: Depends on Phase 3 (specifically T011 — the chart YAML must be real before the drift test can pass on first run).
- **User Story 3 (Phase 5)**: Depends on Phase 3 (chart YAML must be real before `helm install` produces a CRD).
- **Polish (Phase 6)**: Depends on all user stories complete.

### User Story Dependencies (graph)

```text
Setup → Foundational → US1 ──┬─→ US2 → Polish
                             └─→ US3 ┘
```

US2 and US3 can run in parallel after US1 completes (they touch different files and have no inter-dependency).

### Within Each User Story

- US1: T008 → T009 → T010 → T011 → T012 (sequential; same `main.rs` for T008/T009; T011 depends on T009; T010 is verification)
- US2: T013 → T014 → T015 → T016 (sequential; T013 and T014 edit the same file; T015 runs the suite; T016 is end-to-end verification)
- US3: T017 → T018 (sequential; T018 documents what T017 added)

### Parallel Opportunities

- **Phase 1**: T001 ∥ T002 (different Cargo.toml files).
- **Phase 2**: T003 ∥ T004 (different files; T004 only requires T003 to exist for the use paths to resolve, but the edit itself is independent).
- **Phase 5 & 6 (US3 & Polish)**: After US1 completes, US2 and US3 are independent; T019 ∥ T020 within Polish.

---

## Parallel Example: Phase 1 + early Phase 2

```bash
# Two-developer split of foundational work:
Task: "T001 Add pretty_assertions = \"1\" to crates/operator/Cargo.toml [dev-dependencies]"
Task: "T002 Add operator path dep to crates/ctl/Cargo.toml [dependencies]"
Task: "T003 Create crates/operator/src/lib.rs with pub mod declarations"
Task: "T004 Remove inline mod declarations from crates/operator/src/main.rs"
```

---

## Implementation Strategy

### MVP (just User Story 1)

1. Complete Phase 1 (Setup) and Phase 2 (Foundational).
2. Complete Phase 3 (US1): the `mikebom-operator-ctl crd` command exists, runs, and was used to populate the chart YAML.
3. **STOP and VALIDATE**: contributors can regenerate the chart YAML. The chart YAML is real.

At this checkpoint, the regen command is functional but unenforced. Helm chart consumers benefit (chart now installs a real CRD), but drift can still creep in.

### Recommended full delivery (US1 + US2 + US3 + Polish)

1. Setup + Foundational → checkpoint.
2. US1 → checkpoint (MVP).
3. US2 → checkpoint (drift now enforced; this is the constitution-VII milestone).
4. US3 → checkpoint (kind-E2E asserts chart installs).
5. Polish → checkpoint (docs + pre-PR gate).
6. PR ready.

### Parallel team split

After Phase 3 completes:
- Developer A: US2 (drift test, `crates/operator/tests/crd_drift.rs`).
- Developer B: US3 (kind E2E, `e2e/tests/crd_install.rs`).
- Developer C: Polish docs (after US2 + US3 land).

---

## Notes

- `[P]` tasks operate on different files and have no incomplete-task dependencies.
- `[Story]` label maps each task to its user story for traceability.
- US1 is the MVP and only required deliverable for "the regen command exists".
- US2 is the constitution-VII enforcement milestone — without it, the feature is documentation, not policy.
- US3 satisfies constitution VI (kind-E2E for CRD-shape changes).
- After each phase, commit before moving to the next so a partial PR remains coherent if the work is paused.
