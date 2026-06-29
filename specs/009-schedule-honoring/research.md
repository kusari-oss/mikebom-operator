# Phase 0 Research: Schedule honoring (cron + interval)

This document records the decisions feature 009 makes before code lands. Each
decision is short: what we're doing, why, and what we considered.

## 1. Cron library

- **Decision**: `cron = "0.12"`. Pure-Rust, ~5M downloads, standard 5-field syntax via `cron::Schedule::from_str(expr).upcoming(Utc).next()`. License: MIT/Apache-2.0.
- **Rationale**: Mature, simple API, no native deps. The `upcoming` iterator handles all the cron arithmetic we need.
- **Alternatives**: `cron_parser` (less mature, ad-hoc maintenance); `croner` (newer, fewer downloads); roll-our-own (rejected — cron parsing has 30+ years of subtle edge cases).

## 2. Duration library

- **Decision**: `humantime = "2.1"`. Single dep, pure-Rust. `humantime::parse_duration("6h")` returns `Result<Duration, _>`. Handles `"6h"`, `"30m"`, `"1h30m"`, etc.
- **Rationale**: Tiny dep, robust parser. Go-style "6h" is its native syntax.
- **Alternatives**: roll-our-own regex (~30 lines, rejected — humantime is smaller); `duration-str` (heavier, more dep tree).

## 3. Schedule evaluation function shape

- **Decision**:
  ```rust
  pub enum ScheduleSpec {
      Cron(cron::Schedule),
      Interval(Duration),
  }

  pub fn parse_schedule(spec: &crds::Schedule) -> Result<ScheduleSpec, ScheduleError>;

  pub fn compute_next_scheduled_time(
      schedule: &ScheduleSpec,
      anchor: DateTime<Utc>,
      jitter: Duration,
  ) -> DateTime<Utc>;

  pub fn is_schedule_due(next_scheduled: DateTime<Utc>, now: DateTime<Utc>) -> bool {
      now >= next_scheduled
  }
  ```

  `parse_schedule` is total — it covers all 4 valid/invalid combinations of `(cron, interval)` plus malformed expressions. `compute_next_scheduled_time` adds jitter to the schedule-computed instant. `is_schedule_due` is a comparison.

- **Rationale**: Pure functions; trivially testable; one source of truth for validation. Schedule + anchor + jitter are the only inputs the function ever needs.
- **Alternatives**: Bundle the anchor-fallback logic inside `compute_next_scheduled_time` (rejected — couples the function to status-shape; caller can resolve the anchor cleanly before calling).

## 4. Schedule validation lives in `desired_status`

- **Decision**: Extend `crate::status::desired_status` to call `scheduler::parse_schedule(&spec.schedule)`. On `Err`, set `Ready=False / reason=InvalidSpec` (same reason as feature 002's target-validity failure) with a message that names the schedule error variant. Feature 002's `InvalidSpec` reason is reused — not extended with new variants — to keep the user-facing reason vocabulary stable.
- **Rationale**: One unified `InvalidSpec` reason for any "your CR is malformed" case. The *message* is where specific guidance goes. Avoids reason-vocabulary churn.
- **Alternatives**: Add `REASON_INVALID_SCHEDULE` (rejected — too granular; admins remember a small number of reasons better than many). Validate schedule deep in the scheduler module only (rejected — desired_status is the centralized validity check; should remain comprehensive).

## 5. Terminal-Job cleanup uses per-Job Foreground deletion

- **Decision**:
  ```rust
  pub async fn cleanup_terminal_jobs(api: &Api<Job>, owned: &[Job]) -> Result<usize, kube::Error> {
      let mut deleted = 0;
      for job in owned {
          if is_job_succeeded(job) || is_job_finally_failed(job) {
              let name = job.metadata.name.as_ref().unwrap();
              api.delete(name, &DeleteParams {
                  propagation_policy: Some(PropagationPolicy::Foreground),
                  ..Default::default()
              }).await.map_err(...)?;
              deleted += 1;
          }
      }
      Ok(deleted)
  }
  ```

  Foreground propagation ensures the Job's pods are cleaned up before the Job object is removed. Each Job deletion is its own API call (per-Job, not a label-selector `delete_collection`).
- **Rationale**: Foreground prevents orphan pods. Per-Job iteration lets us filter on `is_job_succeeded(j) || is_job_finally_failed(j)` — `delete_collection` with a label selector would also delete in-progress Jobs (violates FR-011).
- **Alternatives**: `delete_collection` with a label + field selector for `status.succeeded=1` (rejected — Kubernetes field selectors don't support `status.*` reliably across versions); `delete_collection` with Background propagation (rejected — same in-progress filter problem + orphan-pod risk).

## 6. Reconcile-flow integration

- **Decision**: In `reconcile()`, after feature 008's `status_with_aggregated_outcome` produces `new_status`:
  1. Parse the schedule once (handle `Err` by deferring to `desired_status`'s InvalidSpec path on the next reconcile).
  2. Resolve the anchor: `parse_rfc3339(new_status.last_scan_completed_at)` or fallback to `cr.metadata.creation_timestamp`.
  3. Compute `next_scheduled = compute_next_scheduled_time(schedule, anchor, jitter(cr.uid))`.
  4. Set `new_status.next_scheduled_scan_at = Some(next_scheduled.to_rfc3339())`.
  5. If `new_status.conditions[Ready].reason ∈ {ScanCompleted, ScanFailed}` AND `is_schedule_due(next_scheduled, now)`: call `cleanup_terminal_jobs(...)` and short-requeue (Action::requeue(5s)) so the next cycle picks up the empty-Job-list state.
  6. Patch `new_status` to the CR.
- **Rationale**: Single decision point per reconcile; pure scheduler + one I/O call for cleanup; observable `next_scheduled_scan_at` for admins.
- **Alternatives**: Do the delete + respawn in one reconcile (rejected — would re-implement feature 007's `ensure_jobs` logic; idempotency makes the two-cycle path correct and simpler).

## 7. Per-CR jitter strategy

- **Decision**: `cr_uid_jitter_seconds(uid: &str) -> u64` returns `(blake3::hash(uid.as_bytes()).as_bytes()[0] as u64 * 256 + .as_bytes()[1] as u64) % 60`. Deterministic, bounded 0–59 seconds, cheap.
- **Rationale**: Same CR → same offset across operator restarts (no surprise drift). Different CRs hash to spread evenly across the minute. Prevents thundering herd at `cron: "0 * * * *"` boundaries — 100 CRs spread across 60s = 1.67 creates/sec instead of 100 at the boundary.
- **Alternatives**: Random offset per CR (rejected — non-deterministic confuses ops monitoring). Time-of-day-derived offset (rejected — same offset cluster-wide). Hash on `metadata.name` (rejected — names can collide across namespaces; `uid` is globally unique).

  Implementation note: `blake3` may not be a workspace dep yet. If not, fall back to `sha2` (already a workspace dep via feature 003); the hash quality difference is irrelevant for a mod-60 bucketing.

## 8. Requeue cadence tightening

- **Decision**:
  ```rust
  let until_next = next_scheduled.signed_duration_since(now);
  let requeue = if until_next < ChronoDuration::minutes(1) && until_next > ChronoDuration::zero() {
      until_next.to_std().unwrap() + Duration::from_secs(1)
  } else {
      Duration::from_secs(300) // existing 5-minute heartbeat
  };
  Action::requeue(requeue)
  ```
  Reconcile fires just after the scheduled time when imminent; falls back to the 5-min heartbeat otherwise.
- **Rationale**: Tight scheduling without overhead. A CR with `interval: "1h"` reconciles ~12 times/hour for `lastReconciledAt` heartbeat refresh (cheap), then fires precisely at the hour boundary + jitter.
- **Alternatives**: Always requeue at `next_scheduled` (rejected — skips heartbeat refresh for CRs whose next-scan is hours away; admins lose the "is the operator alive?" signal). Always 5-min requeue (rejected — adds up to ~5min of latency to every schedule, blowing SC-001/SC-002 budgets).

## Cross-feature compatibility

- **Feature 007**: `ensure_jobs` is the spawn path. Feature 009 doesn't modify it; we only delete terminal Jobs and let feature 007's deterministic-naming + 409-as-success logic produce fresh Jobs idempotently.
- **Feature 008**: `aggregate_job_outcomes` returns `StillRunning` when the owned-Job list is empty (the moment after cleanup, before respawn). Feature 008's status mapper preserves the previous-cycle's `Scanning` from feature 007's `Spawned` arm. The transition `ScanCompleted → Scanning → ScanCompleted` over two reconciles is the intended UX.
- **Feature 008's `merge_scanned_images_append_only`**: re-scanned image gets a new `completed_at`; same `image_ref` → newest-wins; no duplicates. Free FR-009 satisfaction.
