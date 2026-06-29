---

description: "Task list for feature 009: schedule honoring (cron + interval)"
---

# Tasks: Schedule honoring (cron + interval)

**Input**: Design documents from `/specs/009-schedule-honoring/`

**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/scheduler.md, quickstart.md

**Tests**: Included. Same TDD-style discipline as features 001–008 — unit tests inline with implementation modules, integration tests under `e2e/` gated by `MIKEBOM_OPERATOR_E2E=1`. Tests are written first within each phase.

**Organization**: Tasks grouped by user story. US1 (Cron) and US2 (Interval) are both P1 and share most infrastructure (everything in Phase 2). US3 (Restart catch-up) is P2 — adds the "exactly one catch-up" guarantee.

## Format: `[ID] [P?] [Story?] Description with file path`

- **[P]**: Can run in parallel (different files, no in-flight dependency).
- **[Story]**: Required on Phase 3+ tasks (US1 / US2 / US3).

## Path Conventions

Rust workspace with the `operator` crate at `crates/operator/`. The `e2e` crate at `e2e/`. All paths are repo-relative.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Add new direct deps, carve out the new module home, extend the CRD struct.

- [X] T001 Add `cron = "0.12"` and `humantime = "2.1"` to the workspace `Cargo.toml` `[workspace.dependencies]` table, then reference both as `cron.workspace = true` and `humantime.workspace = true` in `crates/operator/Cargo.toml`
- [X] T002 Create empty `scheduler` module skeleton at `crates/operator/src/reconcile/scheduler.rs` and register it in `crates/operator/src/reconcile/mod.rs`
- [X] T003 Add `next_scheduled_scan_at: Option<String>` field to `NamespaceScanStatus` in `crates/operator/src/crds/namespace_scan.rs` with `#[serde(default, skip_serializing_if = "Option::is_none")]` (the struct already has `#[serde(rename_all = "camelCase")]` from feature 002, which handles the JSON `nextScheduledScanAt` rename automatically). **Immediately regenerate the chart CRD YAML** (closes L1 from /speckit-analyze — keeps drift check green throughout Phases 2–5): run `cargo run --bin mikebom-operator-ctl -- crd --output charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml` and confirm `cargo test --test crd_drift` passes. T037 in Phase 6 becomes a final verification that no further struct edits introduced drift.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Type system + pure helpers. Both US1 (Cron) and US2 (Interval) depend on `parse_schedule`, `compute_next_scheduled_time`, `is_schedule_due`, and `cr_uid_jitter_seconds` — these MUST land before either story can be exercised.

**⚠️ CRITICAL**: No user story work can begin until this phase is complete.

**Tests first (unit):**

- [X] T004 [P] Unit test for `parse_schedule` with valid cron `"0 */6 * * *"` → `Ok(ScheduleSpec::Cron(_))` in `crates/operator/src/reconcile/scheduler.rs::tests`
- [X] T005 [P] Unit test for `parse_schedule` with valid interval `"6h"` → `Ok(ScheduleSpec::Interval(Duration::from_secs(21600)))` in `crates/operator/src/reconcile/scheduler.rs::tests`
- [X] T006 [P] Unit test for `parse_schedule` with invalid cron `"every 6 hours"` → `Err(ScheduleError::InvalidCron(_))` (English-language phrase rejected) in `crates/operator/src/reconcile/scheduler.rs::tests`
- [X] T007 [P] Unit test for `parse_schedule` with invalid interval `"6 hours"` and `"-1h"` and `"0s"` → `Err(ScheduleError::InvalidInterval(_))` in `crates/operator/src/reconcile/scheduler.rs::tests`
- [X] T008 [P] Unit test for `parse_schedule` with `Schedule { cron: Some, interval: Some }` → `Err(ScheduleError::BothSet)` and `Schedule { cron: None, interval: None }` → `Err(ScheduleError::NeitherSet)` in `crates/operator/src/reconcile/scheduler.rs::tests`
- [X] T009 [P] Unit test for `parse_schedule` with `interval: "500ms"` (sub-minute) → `Err(ScheduleError::IntervalBelowMinimum(_))` in `crates/operator/src/reconcile/scheduler.rs::tests`
- [X] T010 [P] Unit test for `compute_next_scheduled_time` with `ScheduleSpec::Cron`, anchor at fixed time, jitter=0s → next-tick value matches hand-computed expectation in `crates/operator/src/reconcile/scheduler.rs::tests`
- [X] T011 [P] Unit test for `compute_next_scheduled_time` with `ScheduleSpec::Interval(Duration::from_secs(3600))`, anchor at fixed time, jitter=15s → returns `anchor + 1h + 15s` in `crates/operator/src/reconcile/scheduler.rs::tests`
- [X] T012 [P] Unit test for `cr_uid_jitter_seconds`: deterministic (same uid → same output), bounded (0..60), distinct uids produce distinct outputs in `crates/operator/src/reconcile/scheduler.rs::tests`
- [X] T013 [P] Unit test for `is_schedule_due`: `now >= next_scheduled` → true; `now < next_scheduled` → false in `crates/operator/src/reconcile/scheduler.rs::tests`
- [X] T014 [P] Unit test for `cleanup_terminal_jobs` (using the synthetic-Job fixtures from feature 008): when given a mix of succeeded + finally-failed + in-progress Jobs, the function MUST request deletion of succeeded + finally-failed ONLY, never in-progress (FR-011). Uses kube error injection or a classifier-pattern split (factor out `should_delete_job(job: &Job) -> bool` if needed for unit testability) in `crates/operator/src/reconcile/scheduler.rs::tests`

**Implementation:**

- [X] T015 [P] Define `pub enum ScheduleSpec { Cron(cron::Schedule), Interval(Duration) }` in `crates/operator/src/reconcile/scheduler.rs`
- [X] T016 [P] Define `pub enum ScheduleError { BothSet, NeitherSet, InvalidCron(String), InvalidInterval(String), IntervalBelowMinimum(Duration) }` with `thiserror::Error` derives + the `#[error]` messages from data-model.md in `crates/operator/src/reconcile/scheduler.rs`
- [X] T017 [P] Implement pure helper `pub fn parse_schedule(spec: &Schedule) -> Result<ScheduleSpec, ScheduleError>` per contracts/scheduler.md — total over all `(cron, interval)` combinations; minimum interval = 60s — in `crates/operator/src/reconcile/scheduler.rs`
- [X] T018 [P] Implement pure helper `pub fn compute_next_scheduled_time(schedule: &ScheduleSpec, anchor: DateTime<Utc>, jitter: Duration) -> DateTime<Utc>` per contract — `Cron` uses `s.after(&anchor).next().expect(...)`, `Interval` uses `anchor + d`; both add `jitter` — in `crates/operator/src/reconcile/scheduler.rs`
- [X] T019 [P] Implement pure helper `pub fn is_schedule_due(next_scheduled: DateTime<Utc>, now: DateTime<Utc>) -> bool` (single comparison) in `crates/operator/src/reconcile/scheduler.rs`
- [X] T020 [P] Implement pure helper `pub fn cr_uid_jitter_seconds(uid: &str) -> u64` using `sha2::Sha256::digest(uid.as_bytes())` first 2 bytes interpreted as `u16` modulo 60; same `sha2` workspace dep feature 003 already uses in `crates/operator/src/reconcile/scheduler.rs`
- [X] T021 [P] Implement `pub async fn cleanup_terminal_jobs(api: &Api<Job>, owned: &[Job]) -> Result<usize, kube::Error>` per contract — per-Job Foreground deletion; 404 treated as already-deleted (counts as success but not added to `deleted` count); other errors propagate; calls `is_job_succeeded` / `is_job_finally_failed` from feature 008's `status_aggregator` (re-exported as `pub(crate)` if needed) in `crates/operator/src/reconcile/scheduler.rs`

**Checkpoint**: Module + types + pure helpers + cleanup I/O all exist. Unit tests pass against the new code. User stories can now layer integration on top.

---

## Phase 3: User Story 1 — Cron-scheduled re-scan (Priority: P1) 🎯 MVP

**Story goal**: A CR with `spec.schedule.cron` honors the cron schedule. When the next cron tick arrives, terminal Jobs are deleted, fresh Jobs spawn, the CR cycles through `Scanning → ScanCompleted` again.

**Independent Test**: Install the operator into kind. Apply a CR with `cron: "*/2 * * * *"` (every 2 minutes). Wait for the first scan to complete. Within ~30 seconds of the next 2-minute boundary, observe fresh Jobs spawned + the CR cycling through `Scanning → ScanCompleted` with `lastScanCompletedAt` advanced. (Maps to SC-001, FR-001, FR-003.)

**Implementation:**

- [X] T022 [US1] Extend `crate::status::desired_status` to call `scheduler::parse_schedule(&spec.schedule)`; on `Err`, set `Ready=False / reason=InvalidSpec` with the `ScheduleError`'s `Display` as the message (reuses existing `REASON_INVALID_SPEC` per research §4) in `crates/operator/src/status.rs`
- [X] T023 [US1] Unit test in `crates/operator/src/status.rs::tests` that `desired_status` with `Schedule { cron: Some("every 6 hours"), interval: None }` produces `reason=InvalidSpec` + a message containing `"InvalidCron"` or the cron crate's error text
- [X] T024 [US1] Unit test in `crates/operator/src/status.rs::tests` that `desired_status` with `Schedule { cron: None, interval: None }` produces `reason=InvalidSpec` + message containing `"neither cron nor interval set"`
- [X] T025 [US1] In `reconcile()`, after `status_with_aggregated_outcome` produces the final status, if the reason is `ScanCompleted` OR `ScanFailed`: (a) parse the schedule (skip if InvalidSpec — `desired_status` already caught it), (b) resolve the anchor from `status.last_scan_completed_at` or `cr.metadata.creation_timestamp` fallback, (c) compute `next_scheduled` with jitter from `cr.metadata.uid`, (d) set `new_status.next_scheduled_scan_at = Some(next_scheduled.to_rfc3339())`, (e) if `is_schedule_due(next_scheduled, now)`, call `cleanup_terminal_jobs` and short-requeue (5s), (f) **emit `tracing::info!(event = "schedule_decision", namespace_scan = %name, last_scan_completed_at = ?, next_scheduled = ?, decision = ?, …)` after computing the decision, regardless of whether it fires** (closes M3 from /speckit-analyze; pairs with T036's grep check for FR-014) — in `crates/operator/src/reconcile/namespace_scan.rs`
- [X] T025b [US1] Unit test in `crates/operator/src/reconcile/namespace_scan.rs::tests` (closes H1 from /speckit-analyze; verifies FR-010 + SC-007): construct a synthetic CR fixture with `status.last_scan_completed_at` set to a known past time, invoke the reconciler's status-build path (or factor the schedule-decision logic into a pure helper called from reconcile and unit-test the helper directly), assert that the resulting `NamespaceScanStatus.next_scheduled_scan_at` is `Some(rfc3339_string)` AND parses as a `DateTime<Utc>` strictly greater than the current time. If a pure helper extraction is needed for testability, that's expected — the contract in research §6 already implies pure schedule-decision logic.
- [X] T025c [US1] Anti-regression grep verification of the FR-006 in-progress gate (closes M1 from /speckit-analyze): `grep -c "REASON_SCAN_COMPLETED\\|REASON_SCAN_FAILED" crates/operator/src/reconcile/namespace_scan.rs` returns ≥ 2 (one for the structural gate that guards `cleanup_terminal_jobs`, plus possibly references in `status_with_aggregated_outcome` invocation already imported from feature 008). Visual confirmation that `cleanup_terminal_jobs` is called ONLY inside an `if (reason == REASON_SCAN_COMPLETED || reason == REASON_SCAN_FAILED)` block. The static check prevents a refactor from silently removing the gate; FR-006 would be violated if `cleanup_terminal_jobs` fired while owned Jobs were still in progress. — verification on `crates/operator/src/reconcile/namespace_scan.rs`
- [X] T026 [US1] Tighten requeue cadence: when `next_scheduled - now < 1 minute AND > 0`, requeue at `(next_scheduled - now) + 1s`; else keep the 5-minute heartbeat from feature 002 in `crates/operator/src/reconcile/namespace_scan.rs`

**Tests last (E2E, gated):**

- [X] T027 [US1] Add gated kind E2E `t_cron_rescan_within_30s` (uses feature 008's `e2e/tests/common/mod.rs` scaffolding) — installs operator, applies CR with `cron: "*/2 * * * *"` + 1 fixture pod, waits for the first scan to complete (patches the spawned Job to `succeeded=1` to short-circuit), records `lastScanCompletedAt`, waits until the next 2-minute tick + 30s budget, asserts a NEW Job has been spawned (different Job name from the first) and `lastScanCompletedAt` has advanced — in new file `e2e/tests/schedule_honoring.rs`

**Checkpoint**: After Phase 3, cron-driven re-scan works end-to-end. The MVP slice ships.

---

## Phase 4: User Story 2 — Interval-scheduled re-scan (Priority: P1)

**Story goal**: A CR with `spec.schedule.interval` honors the duration. `lastScanCompletedAt + interval + jitter` is the next fire time. Same delete-then-respawn cycle as US1.

**Independent Test**: With Phase 3 landed, apply a CR with `interval: "2m"`. Wait for the first scan, then within `lastScanCompletedAt + 2m + ~30s`, observe a fresh re-scan cycle. (Maps to SC-002, FR-001, FR-004, FR-007.)

**Tests first (unit):**

- [X] T028 [P] [US2] Unit test in `crates/operator/src/reconcile/scheduler.rs::tests` that `compute_next_scheduled_time` with `ScheduleSpec::Interval(60s)` + anchor at `2026-06-29T14:00:00Z` + jitter=10s returns `2026-06-29T14:01:10Z` (interval-anchored math)
- [X] T029 [P] [US2] Unit test that for a `lastScanCompletedAt`-unset CR, the anchor falls back to `cr.metadata.creationTimestamp` — covered by a reconcile-level unit test or by exercising the anchor-resolution helper directly in `crates/operator/src/reconcile/namespace_scan.rs::tests`

**Tests last (E2E, gated):**

- [X] T030 [US2] Add gated kind E2E `t_interval_rescan_within_30s` — applies CR with `interval: "2m"`, same setup as T027 but verifies the interval-based timing — in `e2e/tests/schedule_honoring.rs`
- [X] T031 [US2] Add gated kind E2E `t_both_set_is_invalid_spec` — applies a CR with `cron: "0 * * * *"` AND `interval: "1h"`, asserts `reason=InvalidSpec` within 10s, message names the conflict (FR-005) — in `e2e/tests/schedule_honoring.rs`
- [X] T032 [US2] Add gated kind E2E `t_schedule_edit_takes_effect_on_next_reconcile` — applies CR with `interval: "1h"`, reaches `ScanCompleted`, patches `spec.schedule.interval` to `"2m"`, asserts the next re-scan fires within `lastScanCompletedAt + 2m + 30s` (NOT 1h — FR-015 + SC-006) — in `e2e/tests/schedule_honoring.rs`

**Checkpoint**: After Phase 4, both cron and interval schedules work. Schedule-edit semantics verified.

---

## Phase 5: User Story 3 — Operator restart catches up on missed schedules (Priority: P2)

**Story goal**: An operator that was down for 3 missed schedule windows fires exactly **one** catch-up scan on recovery — not 3 redundant scans.

**Independent Test**: Apply a CR with `interval: "1m"`. Wait for it to reach `ScanCompleted`. Scale the operator deployment to 0 replicas. Wait 5 minutes (= 5 missed windows). Scale back to 1 replica. Within 30s of the new pod becoming Ready, observe exactly **one** new Job creation (verifiable by counting Job-creation events or by listing live Jobs and asserting `len == 1` for the in-scope image). (Maps to SC-005, FR-008.)

**Implementation note**: this story is largely *verifying* that the existing implementation (T025's reconcile flow) naturally satisfies "exactly one catch-up." The schedule check is anchor-relative (`is_schedule_due(next_scheduled, now)` is `true` once `now > next_scheduled`, regardless of how far past — it's a single boolean, not a loop). On recovery, the first reconcile per CR sees `next_scheduled` is way in the past, fires one delete+respawn cycle, and then advances `lastScanCompletedAt` (when the new scan completes), which moves `next_scheduled` forward — no further catch-ups fire.

- [X] T033 [US3] Unit test in `crates/operator/src/reconcile/scheduler.rs::tests` that `compute_next_scheduled_time` with `ScheduleSpec::Interval(60s)` + anchor 5 minutes in the past + jitter=0 returns `anchor + 60s` (5 minutes in the past), and `is_schedule_due` returns `true` exactly once — there's no iteration to "catch up multiple windows"; the natural single-decision-per-reconcile structure provides FR-008's guarantee for free

**Tests last (E2E, gated):**

- [X] T034 [US3] Add gated kind E2E `t_restart_catchup_fires_exactly_once` — applies CR with `interval: "1m"`, reaches `ScanCompleted`, scales operator deployment to 0 replicas, waits 5 minutes (= 5 missed windows), scales back to 1, polls for the new operator pod Ready, then within 60s asserts exactly **one** new Job exists for the in-scope image (counts Jobs with the CR's `kusari.dev/namespace-scan` label that weren't there before the restart) — in `e2e/tests/schedule_honoring.rs`

**Checkpoint**: After Phase 5, operator restart no longer triggers a Job explosion.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Pre-PR gate, anti-regression, drift check, docs.

- [X] T035 [P] Grep verification that `crates/operator/src/reconcile/scheduler.rs` does NOT read any SBOM-content path: fail if `serde_json::from_str` (against an SBOM payload), `cyclonedx`, `spdx`, or `fs::read.*workdir/out` appears (constitution II)
- [X] T036 [P] Grep verification that schedule_decision events are emitted: `grep -c 'event = "schedule_decision"' crates/operator/src/reconcile/namespace_scan.rs` returns ≥ 1 (FR-014)
- [X] T037 Final drift verification — re-run `cargo run --bin mikebom-operator-ctl -- crd --output charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml` and confirm `git diff` is **empty** (no further drift since T003's initial regen). `cargo test --test crd_drift` MUST pass. The chart CRD YAML reflects every CRD struct change from this PR (constitution VII).
- [X] T038 Run pre-PR gate: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. Confirm all feature 001–008 tests still pass (FR-012, FR-013).
- [X] T039 [P] Update `docs/crd-reference.md`: add a new "Scheduling" subsection documenting cron + interval semantics (recipes match `quickstart.md`'s recipe book); document `status.nextScheduledScanAt`
- [X] T040 [P] Update `docs/architecture.md`: small note that feature 009 makes `spec.schedule` consequential; reference the new `status.nextScheduledScanAt` field
- [X] T041 [P] Run gated E2E locally if kind is available: `MIKEBOM_OPERATOR_E2E=1 cargo test --test schedule_honoring` — all 5 new tests (T027, T030, T031, T032, T034) green. Without kind, the gated tests skip cleanly.

---

## Dependencies

```
Phase 1 (Setup: T001 → T002 → T003)
  ↓
Phase 2 (Foundational): T004–T021
  - Tests T004–T014 are [P] (independent fixtures)
  - Impl T015–T021 are [P] (independent functions, same file)
  - Tests should write first, impl next — within-phase ordering
  ↓
Phase 3 (US1 Cron, MVP):
  T022 (desired_status extension) ─┐
                                   ├→ T023, T024 (unit tests verify) [P]
                                   ↓
                                 T025 (reconcile wiring + schedule_decision log)
                                   ↓
                                 T025b (unit test nextScheduledScanAt populated + future)
                                   ↓
                                 T025c (FR-006 gate grep verification)
                                   ↓
                                 T026 (requeue cadence)
                                   ↓
                                 T027 (E2E cron)
  ↓
Phase 4 (US2 Interval): T028, T029 [P] (unit) → T030, T031, T032 [P] (E2E)
  ↓
Phase 5 (US3 Restart catch-up): T033 (unit) → T034 (E2E)
  ↓
Phase 6 (Polish): T035, T036, T039, T040, T041 [P]; T037 sequential (drift regen); T038 sequential (gate)
```

**Story independence**: US2 depends on Phase 2's `parse_schedule` interval arm; both US1 and US2 share T022/T025/T026 since they're shared reconcile-flow infrastructure. The user-story split is in the *tests* (cron-specific vs interval-specific scenarios), not the implementation. US3 is purely an additional E2E with no new implementation — the design naturally satisfies "exactly one catch-up."

## Parallel execution opportunities

- Phase 2 unit tests (T004–T014): 11 tests in the same module's `#[cfg(test)] mod tests` — file-level conflict only on simultaneous writes; safe in 11-way logical parallel via distinct test names.
- Phase 2 impl helpers (T015–T021): 7 functions in the same file — same as above.
- Phase 3 unit tests (T023, T024): 2 tests in `status.rs::tests` — fully parallel.
- Phase 4 E2E tests (T030–T032): 3 tests in the same E2E file — sequential file write but independent semantically.
- Phase 6 (T035, T036, T039, T040, T041): 5 read-only or doc tasks across different paths — fully parallel.

## Implementation strategy

**MVP scope**: Phases 1–3 (end of T027). After Phase 3, cron-driven re-scan works end-to-end. The single biggest UX win of feature 009 — schedules finally do something — is delivered.

**Incremental delivery**:
- After Phase 3: cron schedules work; admins relying on cron expressions are unblocked.
- After Phase 4: interval schedules + schedule-edit + invalid-schedule rejection all work.
- After Phase 5: operator restart no longer surprises admins with redundant scans.
- After Phase 6: pre-PR gate passes; ready for fork-based PR.

**Test counts to expect** (cumulative, on top of features 001–008's 99 lib + 3 main.rs + 2 drift + 24 E2E):
- Phase 2 unit: +11 (T004–T014) → 110 lib tests
- Phase 3 unit: +3 (T023, T024, T025b) → 113 lib tests
- Phase 4 unit: +2 (T028, T029) → 115 lib tests
- Phase 5 unit: +1 (T033) → 116 lib tests
- Phase 3+4+5 gated E2E: +5 (T027, T030, T031, T032, T034) — all skip cleanly without `MIKEBOM_OPERATOR_E2E=1`.

## Format validation

All 43 tasks follow the format `- [ ] T### [P?] [Story?] Description with file path`. User-story phases (T022–T034, plus T025b and T025c inserted during the /speckit-analyze remediation pass) carry `[US1]`/`[US2]`/`[US3]` labels. Setup, foundational, and polish phases carry no story label. Every task names ≥1 exact file path under `crates/operator/`, `e2e/`, `docs/`, `charts/`, or `Cargo.toml`.
