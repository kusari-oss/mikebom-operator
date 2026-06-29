# Phase 1 Data Model: Schedule honoring (cron + interval)

Records the Rust types added/modified by feature 009 and the FR → test mapping.
One additive CRD field; everything else is internal to the `operator` crate.

## New types

### `crate::reconcile::scheduler::ScheduleSpec`

```rust
#[derive(Debug, Clone)]
pub enum ScheduleSpec {
    /// Parsed cron expression (5-field, UTC).
    Cron(cron::Schedule),
    /// Go-style duration (validated as >= 1 minute).
    Interval(Duration),
}
```

Not `#[non_exhaustive]` — internal. `cron::Schedule` itself doesn't impl
`PartialEq`/`Eq`, so callers compare by re-parsing if needed.

### `crate::reconcile::scheduler::ScheduleError`

```rust
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScheduleError {
    #[error("spec.schedule has both cron and interval set; exactly one is required")]
    BothSet,
    #[error("spec.schedule has neither cron nor interval set; exactly one is required")]
    NeitherSet,
    #[error("spec.schedule.cron is not a valid cron expression: {0}")]
    InvalidCron(String),
    #[error("spec.schedule.interval is not a valid Go-style duration: {0}")]
    InvalidInterval(String),
    #[error("spec.schedule.interval must be at least 1 minute (got {0:?})")]
    IntervalBelowMinimum(Duration),
}
```

The `Display` impls drive the `InvalidSpec` status message text.

## Modified types

### `crate::crds::namespace_scan::NamespaceScanStatus` (CRD-visible)

```rust
#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NamespaceScanStatus {
    #[serde(default)]
    pub conditions: Vec<StatusCondition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reconciled_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_scan_completed_at: Option<String>,
    #[serde(default)]
    pub scanned_images: Vec<ScannedImage>,
    /// **NEW in feature 009**: RFC 3339 timestamp of the next scheduled scan
    /// for this CR. Computed from spec.schedule + last_scan_completed_at +
    /// per-CR jitter. Populated on every reconcile that produces a terminal
    /// scan state (ScanCompleted or ScanFailed). Always in the future.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_scheduled_scan_at: Option<String>,
}
```

Additive v1alpha1 per constitution IV. The serde attributes match the existing
pattern (default-empty, omit on serialize when None). Feature 001's drift check
regenerates the chart YAML automatically.

### Status reason constants (`crate::status`) — unchanged

No new constants. Schedule validation reuses the existing `REASON_INVALID_SPEC`
from feature 002. The InvalidSpec message text differentiates schedule errors
from target errors.

## State transitions (extends feature 008's diagram)

```
  ┌──────────────────┐
  │  no status yet   │
  └────────┬─────────┘
           │ first reconcile
           ▼
  ┌──────────────────┐      schedule    ┌──────────────────┐
  │ NotYetReconciled │ ── invalid OR ─▶ │   InvalidSpec    │  ◀── terminal
  └────────┬─────────┘   target invalid └──────────────────┘
           │
           │ schedule + target valid;
           │ ensure_jobs Spawned (feat 007)
           ▼
  ┌──────────────────┐
  │     Scanning     │  ◀──── feat 007 + 008
  └────────┬─────────┘
           │
           │ feat 008 aggregation
           │
           ├─ AllSucceeded ────▶ ScanCompleted ──┐
           │                                     │
           └─ AnyFailed ───────▶ ScanFailed ─────┤
                                                 │
                                                 │ NEW IN FEAT 009:
                                                 │ schedule_due?
                                                 │   yes → delete terminal Jobs
                                                 │         (next reconcile re-enters
                                                 │          feat 007's spawn path)
                                                 │   no  → stay terminal,
                                                 │         update nextScheduledScanAt
                                                 ▼
                                              ┌─────────┐
                                              │ Scanning│  (cycle continues)
                                              └─────────┘
```

`InvalidSpec` is also extended: feature 009's `desired_status` now treats a
malformed schedule as InvalidSpec. The condition path is unchanged from feature
002 — only the *cases* that produce InvalidSpec grow.

## CRD change (constitution IV)

Exactly one additive field: `status.nextScheduledScanAt: Option<String>` (RFC
3339). Optional + `skip_serializing_if = Option::is_none` ensures CRs that
predate feature 009 continue to validate cleanly; the OpenAPI schema gains the
field but doesn't make it required. Feature 001's drift check regenerates the
chart YAML; the PR must include the regenerated file.

## FR → test mapping

| FR | Test |
|---|---|
| FR-001 | E2E (gated): tight cron schedule fires re-scan within 30s of tick. |
| FR-002 | Unit: `cleanup_terminal_jobs` deletes only succeeded + finally-failed. Integration: full cycle ScanCompleted → delete → Scanning → ScanCompleted. |
| FR-003 | Unit: `parse_schedule` over valid cron, invalid cron, "every 6 hours" (English), empty string. |
| FR-004 | Unit: `parse_schedule` over `"6h"`, `"30m"`, `"1h30m"`, `"6 hours"` (invalid), `"-1h"` (invalid), `"0s"` (invalid), `"500ms"` (below minimum). |
| FR-005 | Unit: `parse_schedule(Schedule { cron: Some("..."), interval: Some("...") })` → `BothSet`; `Schedule { cron: None, interval: None }` → `NeitherSet`. Integration: applying a both-set CR transitions to InvalidSpec within 10s. |
| FR-006 | Unit: when called with reason=Scanning, the reconciler's schedule-due path is gated off (no delete fires). Integration: a long-running scan doesn't get a second concurrent generation when the schedule elapses mid-scan. |
| FR-007 | Unit: `compute_next_scheduled_time` with `anchor=lastScanCompletedAt`; same with `anchor=creationTimestamp` (when `lastScanCompletedAt` is None). |
| FR-008 | Integration: `kubectl scale deployment mikebom-operator --replicas=0`, wait 3 missed windows, `--replicas=1`, observe exactly 1 catch-up scan (count Job creations via log analysis or `kubectl get events`). |
| FR-009 | Inherited from feature 008 (`merge_scanned_images_append_only`). |
| FR-010 | Unit: `status.next_scheduled_scan_at` is set on reconcile completion. Integration: `kubectl get namespacescan -o jsonpath='{.status.nextScheduledScanAt}'` returns a future timestamp. |
| FR-011 | Unit: `cleanup_terminal_jobs` skips Jobs with `succeeded < 1` AND not finally-failed. |
| FR-012/FR-013 | Inherited: feature 002 / 007 / 008 tests pass unchanged; drift check regenerates and stays green. |
| FR-014 | Static: structured log event `schedule_decision` emitted with CR name, last completion, next scheduled, decision (fire/defer). |
| FR-015 | Integration: edit `spec.schedule.interval` from `"1h"` to `"15m"` on a CR at `ScanCompleted`; verify next re-scan fires within 15m+30s of the edit. |

## Helper contracts

See [contracts/scheduler.md](./contracts/scheduler.md) for the per-function
invariants and non-goals.
