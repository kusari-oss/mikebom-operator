# Implementation Plan: Schedule honoring (cron + interval)

**Branch**: `009-schedule-honoring` | **Date**: 2026-06-29 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/009-schedule-honoring/spec.md`

## Summary

Make `spec.schedule.{cron, interval}` (in the CRD since feature 001) *consequential* for the first time. A new `crate::reconcile::scheduler` module computes "next scheduled scan time" from the CR's schedule + `status.lastScanCompletedAt` (or `metadata.creationTimestamp` for never-yet-completed CRs). The reconciler, after feature 008's aggregation produces a terminal scan state (`ScanCompleted` or `ScanFailed`), checks whether the next scheduled time has elapsed. If so, it deletes the CR's terminal Jobs (succeeded + finally-failed); the next reconcile cycle's `ensure_jobs` (feature 007) sees zero owned Jobs and respawns the in-scope image set, naturally transitioning the CR back through `Scanning → ScanCompleted`. Validation extends feature 002's `desired_status`: invalid cron, malformed interval, both-fields-set, or neither-set surface as `InvalidSpec`. A new optional CRD field `status.nextScheduledScanAt` (additive v1alpha1) surfaces the next-fire-time to admins for `kubectl get` visibility. Deterministic per-CR jitter (a 0–59s offset hashed from the CR's `metadata.uid`) prevents thundering-herd at cron tick boundaries. No Helm chart changes; CRD shape gains one optional field; drift check regenerates cleanly.

## Technical Context

**Language/Version**: Rust 1.85+ stable (workspace toolchain, same as features 001–008).

**Primary Dependencies**:
- `cron = "0.12"` — **new direct dep** (pure-Rust cron parser + `Schedule::upcoming` iterator). Justified by FR-003 (need correct standard 5-field cron parsing); pure-Rust per constitution I.
- `humantime = "2.1"` — **new direct dep** for Go-style duration parsing (e.g., `"6h"`, `"30m"`). Pure-Rust, single small dep.
- `chrono` 0.4 (existing) — used for `DateTime<Utc>` arithmetic.
- `kube`/`k8s-openapi` (existing) — `Api::<Job>::delete_collection` for terminal-Job cleanup.

**Storage**: N/A — schedule arithmetic is pure; deletion writes to the kube API.

**Testing**:
- **Unit tests** in `crates/operator/src/reconcile/scheduler.rs`: pure-function tests for `parse_schedule`, `compute_next_scheduled_time`, `is_schedule_due`, `cr_uid_jitter_seconds`. Covers cron parsing, interval parsing, invalid expressions, both-set / neither-set rejection, anchor fallback (`lastScanCompletedAt` → `creationTimestamp`), and the jitter function.
- **Unit tests** in `crates/operator/src/status.rs`: extend `desired_status` tests for the new schedule-validity check; verify `InvalidSpec` is set for malformed schedules.
- **Integration test** (new `e2e/tests/schedule_honoring.rs`, gated by `MIKEBOM_OPERATOR_E2E=1`): uses feature 008's `common/mod.rs` chart-install scaffolding. Three tight-schedule tests cover the cron-driven, interval-driven, and operator-restart-catchup paths.
- Existing tests stay green; feature 002 / 007 / 008 tests run unchanged.

**Target Platform**: Linux x86_64 / macOS dev — same as features 001–008.

**Project Type**: Rust workspace — implementation lives in the existing `operator` crate, mostly in a new `reconcile/scheduler.rs` module.

**Performance Goals**:
- Schedule evaluation per reconcile: <1ms (pure-function lookup).
- Re-scan trigger latency: ≤30s from scheduled time (SC-001 / SC-002 budget).
- Cluster-wide herd at minute boundaries: jitter spreads 100 CRs across a 60-second window so the operator's Job-create rate stays under ~2 creates/sec for v0.9's stated scale.

**Constraints**:
- Constitution I/II/V: no new C deps; no SBOM access; scheduler reads only spec.schedule + status.lastScanCompletedAt.
- Constitution IV: one additive CRD field (`status.nextScheduledScanAt`). Feature 001's drift check regenerates from the Rust struct.
- Feature 007's invariant: `ensure_jobs` is idempotent → can't directly respawn. Feature 009's re-scan path satisfies this by *deleting* terminal Jobs first, then letting `ensure_jobs` spawn fresh ones on the next reconcile (naturally cleared by feature 007's deterministic naming).
- Feature 008's `Scanning` aggregation: when terminal Jobs are deleted but no fresh ones exist yet, feature 008's aggregator returns `StillRunning` (empty list branch), and the status stays at `Scanning` (from feature 007's `Spawned` arm). The transition is `ScanCompleted → Scanning → ScanCompleted` over two reconcile cycles, which is the intended UX.

**Scale/Scope**:
- v0.9 target: same 100 CRs × 25 images as features 007/008. Adds schedule-driven re-scan; the herd is bounded by per-CR jitter.
- Cron parsing cost: ~10µs per CR per reconcile (negligible).
- New CRD field is a single optional RFC 3339 string per CR — no scale concerns.

## Constitution Check

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Pure Rust where reasonable | PASS | Two new direct deps (`cron`, `humantime`) — both pure Rust, widely used. Justified by FR-003 / FR-004; no native or C linkage. |
| II. USE not EMBED (NON-NEGOTIABLE) | PASS | Scheduler reads `spec.schedule` + `status.lastScanCompletedAt` only. No SBOM access; terminal-Job deletion is a kube API call, not SBOM manipulation. |
| III. Fail Closed on RBAC (NON-NEGOTIABLE) | PASS | No new RBAC verbs. The existing ClusterRole grants `batch/v1.jobs:get,list,watch,create,delete` — the `delete` verb covers feature 009's terminal-Job cleanup. If `delete` is denied, the operator surfaces `RBACInsufficient` (feature 007's existing semantics; reused). |
| IV. CRD Backward Compatibility | PASS | One additive field: `status.nextScheduledScanAt: Option<String>`. Feature 001's drift check regenerates from the Rust struct; PR must include the regenerated chart YAML to stay green. |
| V. SBOM-Format Agnostic | PASS | No SBOM parsing. Scheduler doesn't read `scannedImages[].sbomLocation` contents. |
| VI. Hermetic E2E Tests (NON-NEGOTIABLE) | PASS | New gated kind E2E reuses feature 008's `common/mod.rs` chart-install scaffolding. Three test scenarios cover cron + interval + operator-restart-catchup. |
| VII. Helm Chart Lockstep | PASS | One CRD field addition triggers drift regeneration (must be in the PR). No template changes; RBAC unchanged. |

All gates pass. No `## Complexity Tracking` section needed.

## Project Structure

### Documentation (this feature)

```text
specs/009-schedule-honoring/
├── plan.md                              # this file
├── spec.md                              # spec (no clarifications were needed)
├── research.md                          # Phase 0: 8 decisions (cron lib, duration lib, schedule eval shape, terminal-Job cleanup, reconcile-flow integration, jitter strategy, requeue cadence tightening, kind E2E approach)
├── data-model.md                        # Phase 1: types added (Schedule validation, ScheduleError, NextScheduledTime), CRD addition, FR→test mapping
├── quickstart.md                        # Phase 1: admin upgrade flow + cron/interval recipe book + contributor extension guide
├── contracts/
│   └── scheduler.md                     # Internal contract: parse_schedule + compute_next_scheduled_time + is_schedule_due
└── tasks.md                             # /speckit-tasks output (not created here)
```

### Source Code (repository root)

```text
crates/operator/src/
├── crds/
│   └── namespace_scan.rs                # MODIFY (small):
│                                        #   - Add `next_scheduled_scan_at: Option<String>` field to `NamespaceScanStatus`
│                                        #   - `#[serde(default, skip_serializing_if = "Option::is_none")]`
│                                        #   - JsonSchema derive picks it up automatically
│
├── reconcile/
│   ├── mod.rs                           # MODIFY: register new `scheduler` submodule
│   ├── namespace_scan.rs                # MODIFY:
│   │                                    #   - After feature 008's aggregation produces a terminal reason
│   │                                    #     (ScanCompleted or ScanFailed), call scheduler::is_schedule_due(...)
│   │                                    #   - If due: call scheduler::cleanup_terminal_jobs(...) to delete owned
│   │                                    #     succeeded + finally-failed Jobs
│   │                                    #   - In all cases, set status.next_scheduled_scan_at via
│   │                                    #     scheduler::compute_next_scheduled_time(...)
│   │                                    #   - Tighten the requeue cadence: if next_scheduled is within 1m, requeue
│   │                                    #     at next_scheduled + jitter; else keep the 5m default
│   ├── scan_orchestrator.rs             # UNCHANGED (feature 007 contract stable)
│   ├── status_aggregator.rs             # UNCHANGED (feature 008 contract stable)
│   └── scheduler.rs                     # NEW:
│                                        #   - `pub enum ScheduleSpec { Cron(cron::Schedule), Interval(Duration) }`
│                                        #   - `pub enum ScheduleError { BothSet, NeitherSet, InvalidCron(String), InvalidInterval(String), IntervalBelowMinimum }`
│                                        #   - `pub fn parse_schedule(spec: &crds::Schedule) -> Result<ScheduleSpec, ScheduleError>`
│                                        #   - `pub fn compute_next_scheduled_time(schedule: &ScheduleSpec, anchor: DateTime<Utc>, jitter: Duration) -> DateTime<Utc>`
│                                        #   - `pub fn is_schedule_due(next_scheduled: DateTime<Utc>, now: DateTime<Utc>) -> bool`
│                                        #   - `pub fn cr_uid_jitter_seconds(uid: &str) -> u64` (0–59s deterministic hash; same uid → same offset)
│                                        #   - `pub async fn cleanup_terminal_jobs(api: &Api<Job>, owned: &[Job]) -> Result<usize, kube::Error>`
│                                        #     (deletes Jobs where is_job_succeeded(j) OR is_job_finally_failed(j))
│
└── status.rs                            # MODIFY (small):
                                         #   - Extend `desired_status` to call new `is_schedule_valid(spec.schedule)`;
                                         #     when invalid, return `InvalidSpec` (same reason as feature 002's target-
                                         #     validity failure) with a schedule-specific message
                                         #   - The schedule-validity check delegates to `scheduler::parse_schedule`
                                         #     and surfaces the error variant in the InvalidSpec message

e2e/tests/
├── schedule_honoring.rs                 # NEW (gated): real-operator-in-kind tests
│                                        #   - 3 scenarios: t_cron_rescan, t_interval_rescan, t_restart_catchup
│                                        #   - Tight schedules (cron="*/2 * * * *", interval="2m") for tractable test runs
│                                        #   - Reuses feature 008's e2e/tests/common/mod.rs (no modification)
└── (others unchanged)

charts/mikebom-operator/
├── crds/namespacescan.kusari.dev_v1.yaml  # REGENERATE (one new field appears in OpenAPI schema)
└── (templates unchanged)                # No RBAC changes; existing jobs:delete grant covers feature 009

docs/
├── crd-reference.md                     # MODIFY: document `status.nextScheduledScanAt`; add a "Scheduling" subsection
│                                        # with cron + interval examples and admin-facing guarantees
└── architecture.md                      # MODIFY: small note that feature 009 makes spec.schedule consequential
```

**Structure Decision**:

The scheduler gets its own sibling under `reconcile/` (peer to `scan_orchestrator.rs` from feature 007 and `status_aggregator.rs` from feature 008), not folded into either. Rationale:

- **Single responsibility**: orchestration spawns Jobs; aggregation interprets them; scheduling decides *when* to redo the cycle. Three different cognitive concerns.
- **Independent testability**: scheduler is fully pure for the schedule-arithmetic side, with a single async helper for terminal-Job deletion. Unit tests don't need a kube client.
- **Future extensibility**: when manual triggers ("scan now" annotation) land later, the trigger logic naturally lives next to scheduling, not orchestration.

`status_with_orchestration_result` and `status_with_aggregated_outcome` stay in `status.rs`. Feature 009's only `status.rs` change is the InvalidSpec extension — small, in line with existing patterns.

## Phase 0: Outline & Research

Research artifact: [research.md](./research.md). The 8 decisions it records:

1. **Cron library**: `cron = "0.12"`. Pure-Rust, widely used, supports `cron::Schedule::upcoming(Utc).next()`. Alternatives considered: `cron_parser` (less mature), `croner` (newer, larger).

2. **Duration library**: `humantime = "2.1"`. Single-dep, pure-Rust, parses `"6h"` / `"30m"` / `"1h30m"`. Alternatives: roll-our-own simple regex parser (rejected — humantime is small and well-tested), `duration-str` (rejected — heavier).

3. **Schedule evaluation function shape**: `compute_next_scheduled_time(schedule, anchor, jitter)` is fully pure. Anchor is `status.lastScanCompletedAt` (parsed back to `DateTime<Utc>`) or `metadata.creationTimestamp` if the former is None. Jitter is added to whichever absolute time the schedule computes (cron-next or anchor+interval).

4. **Schedule validation**: extracted into `parse_schedule(spec: &crds::Schedule) -> Result<ScheduleSpec, ScheduleError>`. Called from both `desired_status` (for InvalidSpec gating) and from the reconcile path (for computing next-fire). One source of truth; same error variants surface to status messages.

5. **Terminal-Job cleanup**: `cleanup_terminal_jobs` uses `Api::<Job>::delete(name, &DeleteParams { propagation_policy: Some(PropagationPolicy::Foreground), .. })` per Job. Foreground propagation ensures the Job's pods are cleaned up before the Job object itself is removed (no orphan pods). Alternative: `delete_collection` with a label selector (simpler API call but doesn't filter by status; we'd delete in-progress Jobs too — rejected, violates FR-011).

6. **Reconcile-flow integration**: after feature 008's aggregator runs and the status reason is determined, the reconciler:
   - Computes `next_scheduled_time` via the scheduler (regardless of reason).
   - If reason ∈ {ScanCompleted, ScanFailed} AND `is_schedule_due(next_scheduled, now)`, calls `cleanup_terminal_jobs` and returns from this reconcile with a *short* requeue (e.g., 5s) so the next cycle picks up the empty-Job-list state and respawns.
   - Always writes `status.next_scheduled_scan_at = next_scheduled.to_rfc3339()` for admin visibility.

7. **Jitter strategy**: `cr_uid_jitter_seconds(uid)` returns `(hash(uid) % 60)` seconds. Deterministic (same CR → same offset across operator restarts), bounded (0–59s), cheap (single SHA-256 or BLAKE3 of the UID, modulo 60). Spreads cluster-wide herd at minute boundaries. Alternatives: random offset (rejected — non-deterministic confuses ops); fixed offset per CR creation order (rejected — adversarial CR creation could pile up).

8. **Requeue cadence tightening**: when `next_scheduled - now < 1m`, the reconciler returns `Action::requeue(next_scheduled - now + 1s)` so the next reconcile fires just after the scheduled time. Outside that window, the existing 5-minute heartbeat (feature 002) is kept. Alternative: always requeue at next_scheduled (rejected — would skip the heartbeat refresh of `lastReconciledAt` for CRs whose next-scan is hours away).

**Output**: research.md with all 8 decisions resolved. No `NEEDS CLARIFICATION` markers remain.

## Phase 1: Design & Contracts

**Prerequisites**: research.md complete.

### Data model

[data-model.md](./data-model.md) captures:

- **`ScheduleSpec` (new, in scheduler module)**: `enum { Cron(cron::Schedule), Interval(Duration) }`. Result of `parse_schedule`. Not `#[non_exhaustive]` — internal.

- **`ScheduleError` (new)**: `enum { BothSet, NeitherSet, InvalidCron(String), InvalidInterval(String), IntervalBelowMinimum }`. Used by `parse_schedule`; the variants drive the InvalidSpec message text.

- **`NamespaceScanStatus` (modified)**: add `next_scheduled_scan_at: Option<String>` field. RFC 3339 string. Additive v1alpha1.

- **CRD shape change**: `status.nextScheduledScanAt` is the only new field. Feature 001's drift check regenerates chart YAML.

- **State transitions** (extends feature 008's state diagram):

  ```
  ScanCompleted ─── schedule_due ───▶ (delete terminal Jobs)
                                         │
                                         ▼
                                      Scanning ◀── feat 007's Spawned arm
                                         │ (fresh Jobs run to completion)
                                         ▼
                                      ScanCompleted ──┐
                                                      │
                                                      │ next scheduled time advanced
                                                      ▼
                                              (loop forever)

  ScanFailed ─── schedule_due ───▶ (same delete + respawn path)
                                         │
                                         ▼
                                      Scanning → ScanCompleted (if fixed)
                                              → ScanFailed (if still broken)
  ```

- **FR → test mapping**:
  - FR-001/SC-001/SC-002 → integration test (gated): schedule fires re-scan within 30s.
  - FR-002 → unit test: `cleanup_terminal_jobs` deletes only succeeded + finally-failed; in-progress Jobs untouched. Integration test verifies the full delete-then-respawn cycle.
  - FR-003/FR-004/FR-005 → unit tests for `parse_schedule` over the cron-invalid / interval-invalid / both-set / neither-set / below-minimum cases.
  - FR-006 → unit test: `is_schedule_due` returns `false` regardless of schedule when the gating reason is `Scanning`. Integration test verifies no second concurrent generation spawns.
  - FR-007 → unit test: anchor fallback (`lastScanCompletedAt` → `creationTimestamp`).
  - FR-008 → integration test: kill operator, wait 3 missed windows, restart, verify exactly 1 catch-up.
  - FR-009 → inherited from feature 008's `merge_scanned_images_append_only` (no new test).
  - FR-010 → unit test: `status.next_scheduled_scan_at` is populated on every reconcile that produces a terminal scan state; value is in the future relative to `lastScanCompletedAt`.
  - FR-011 → unit test: `cleanup_terminal_jobs` skips Jobs with `succeeded < 1 && !finally_failed`.
  - FR-012/FR-013 → inherited (existing tests stay green; drift check regenerates).
  - FR-014 → static: structured logs emitted by reconcile path with `event = "schedule_decision"`.
  - FR-015 → integration test: edit `spec.schedule.interval` from `1h` to `15m` on a CR at `ScanCompleted`; verify next re-scan fires within 15m+30s of the edit.

### Contracts

[contracts/scheduler.md](./contracts/scheduler.md) — internal contract for the scheduler module. Pins:

- `parse_schedule` is total over `(cron, interval)` combinations and returns a deterministic `ScheduleError` variant for each invalid shape.
- `compute_next_scheduled_time` is pure and adds jitter to the schedule-computed instant.
- `is_schedule_due` is pure (comparison only).
- `cleanup_terminal_jobs` is the only I/O surface; its delete operations use Foreground propagation; it returns the count of Jobs deleted.
- `cr_uid_jitter_seconds` is deterministic (same input → same output) and bounded to 0–59 seconds.

### Agent context update

The project's `CLAUDE.md` currently points at feature 008's plan. Phase 1 updates this to feature 009.

**Output**: data-model.md, contracts/scheduler.md, quickstart.md, updated `CLAUDE.md`.

## Re-evaluate Constitution Check (post-design)

| Principle | Status | Notes |
|-----------|--------|-------|
| I | PASS | Two new pure-Rust deps (`cron`, `humantime`). |
| II | PASS | Scheduler reads spec.schedule + status.lastScanCompletedAt; no SBOM access. |
| III | PASS | Existing `jobs:delete` RBAC grant covers terminal-Job cleanup; 403 surfaces as `RBACInsufficient`. |
| IV | PASS | One additive optional field (`status.nextScheduledScanAt`); drift check regenerates. |
| V | PASS | No SBOM access. |
| VI | PASS | New gated kind E2E covers the watch-driven re-scan transition. |
| VII | PASS | No template changes; CRD field addition reflected in chart YAML via the standard regenerate path. |

All gates still pass post-design. No complexity tracking needed.
