//! Schedule honoring for `NamespaceScan` CRs.
//!
//! Makes `spec.schedule.{cron, interval}` consequential. Public surface:
//! `parse_schedule`, `compute_next_scheduled_time`, `is_schedule_due`,
//! `cr_uid_jitter_seconds`, `should_delete_job`, `cleanup_terminal_jobs`.
//!
//! See:
//! - `specs/009-schedule-honoring/contracts/scheduler.md`
//! - `specs/009-schedule-honoring/data-model.md`
//! - `specs/009-schedule-honoring/research.md`

use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, Utc};
use k8s_openapi::api::batch::v1::Job;
use kube::{
    api::{DeleteParams, PropagationPolicy},
    Api,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::crds::namespace_scan::Schedule;
use crate::reconcile::status_aggregator::{is_job_finally_failed, is_job_succeeded};

/// Result of `parse_schedule`. Represents a valid schedule the operator can
/// evaluate for next-fire-time.
#[derive(Debug, Clone)]
pub enum ScheduleSpec {
    /// Parsed cron expression (5-field, UTC). The `cron::Schedule` carries
    /// the AST needed for `.after(...).next()` iteration. Boxed because
    /// `cron::Schedule` is large (~500+ bytes) compared to the other variant.
    Cron(Box<cron::Schedule>),
    /// Go-style duration, already validated as `>= 60s`.
    Interval(Duration),
}

/// Validation errors from `parse_schedule`. The `Display` impls drive the
/// `InvalidSpec` status message text (FR-005 / FR-003 / FR-004).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
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

const MINIMUM_INTERVAL: Duration = Duration::from_secs(60);

// =============================================================================
// Pure helpers — fully unit-testable, no I/O.
// =============================================================================

/// Validate the CR's schedule spec. Total over all `(cron, interval)`
/// combinations (research.md §4).
pub fn parse_schedule(spec: &Schedule) -> Result<ScheduleSpec, ScheduleError> {
    match (spec.cron.as_deref(), spec.interval.as_deref()) {
        (Some(_), Some(_)) => Err(ScheduleError::BothSet),
        (None, None) => Err(ScheduleError::NeitherSet),
        (Some(cron_expr), None) => {
            // The user provides a standard 5-field cron expression (FR-003);
            // the `cron` 0.12 crate expects 7-field (sec min hour dom mon dow year).
            // Prepend "0" (seconds=0) and append "*" (year=*) before parsing.
            // First validate the user's input has exactly 5 fields to avoid
            // accidentally accepting 6/7-field expressions through this path.
            let trimmed = cron_expr.trim();
            let field_count = trimmed.split_whitespace().count();
            if field_count != 5 {
                return Err(ScheduleError::InvalidCron(format!(
                    "expected standard 5-field cron expression (minute hour day-of-month month day-of-week); got {field_count} field(s)"
                )));
            }
            let normalized = format!("0 {trimmed} *");
            cron::Schedule::from_str(&normalized)
                .map(|s| ScheduleSpec::Cron(Box::new(s)))
                .map_err(|e| ScheduleError::InvalidCron(format!("{e}")))
        }
        (None, Some(interval_str)) => {
            let dur = humantime::parse_duration(interval_str)
                .map_err(|e| ScheduleError::InvalidInterval(format!("{e}")))?;
            if dur < MINIMUM_INTERVAL {
                Err(ScheduleError::IntervalBelowMinimum(dur))
            } else {
                Ok(ScheduleSpec::Interval(dur))
            }
        }
    }
}

/// Compute the next scheduled scan instant from the schedule + anchor +
/// per-CR jitter. Pure function (research.md §3).
pub fn compute_next_scheduled_time(
    schedule: &ScheduleSpec,
    anchor: DateTime<Utc>,
    jitter: Duration,
) -> DateTime<Utc> {
    let base = match schedule {
        ScheduleSpec::Cron(s) => s.after(&anchor).next().expect(
            "cron::Schedule::after always yields at least one future tick for 5-field cron",
        ),
        ScheduleSpec::Interval(d) => anchor + chrono::Duration::from_std(*d).unwrap(),
    };
    base + chrono::Duration::from_std(jitter).unwrap()
}

/// `true` iff the current wall-clock time has reached or passed the next
/// scheduled scan time (FR-001 / FR-006 gating).
pub fn is_schedule_due(next_scheduled: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    now >= next_scheduled
}

/// Per-CR jitter seconds, hashed deterministically from the CR's
/// `metadata.uid`. Bounded to `0..60` (research.md §7).
pub fn cr_uid_jitter_seconds(uid: &str) -> u64 {
    let digest = Sha256::digest(uid.as_bytes());
    let bytes = digest.as_slice();
    let n: u64 = (bytes[0] as u64) << 8 | (bytes[1] as u64);
    n % 60
}

/// `true` iff this Job is in a terminal state (succeeded OR finally failed)
/// and is therefore safe for the scheduler to delete during re-scan setup
/// (FR-002 / FR-011 — never deletes in-progress Jobs).
pub fn should_delete_job(job: &Job) -> bool {
    is_job_succeeded(job) || is_job_finally_failed(job)
}

// =============================================================================
// I/O — terminal-Job cleanup.
// =============================================================================

/// Delete every owned Job that `should_delete_job` flags as terminal. Uses
/// Foreground propagation so the Job's pods are cleaned up before the Job
/// object itself is removed (no orphan pods).
///
/// Treats 404 on a per-Job delete as already-deleted (counts that Job as a
/// no-op, doesn't propagate the error). Other kube errors propagate.
///
/// Returns the count of Jobs that were actually deleted (excludes 404 misses
/// and in-progress skips).
pub async fn cleanup_terminal_jobs(api: &Api<Job>, owned: &[Job]) -> Result<usize, kube::Error> {
    let mut deleted = 0usize;
    for job in owned {
        if !should_delete_job(job) {
            continue;
        }
        let Some(name) = job.metadata.name.as_deref() else {
            continue;
        };
        match api
            .delete(
                name,
                &DeleteParams {
                    propagation_policy: Some(PropagationPolicy::Foreground),
                    ..Default::default()
                },
            )
            .await
        {
            Ok(_) => deleted += 1,
            Err(kube::Error::Api(e)) if e.code == 404 => {
                // Already gone (TTL race) — fine, not counted as our delete.
            }
            Err(e) => return Err(e),
        }
    }
    Ok(deleted)
}

// =============================================================================
// Tests.
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crds::namespace_scan::Schedule;
    use k8s_openapi::api::batch::v1::JobSpec;
    use k8s_openapi::api::batch::v1::JobStatus;
    use k8s_openapi::api::core::v1::{Container, EnvVar, PodSpec, PodTemplateSpec};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    // -----------------------------------------------------------------------
    // Fixture helpers
    // -----------------------------------------------------------------------

    fn schedule(cron: Option<&str>, interval: Option<&str>) -> Schedule {
        Schedule {
            cron: cron.map(String::from),
            interval: interval.map(String::from),
        }
    }

    fn anchor() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-29T14:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn job_with_status(succeeded: Option<i32>, failed: Option<i32>, backoff: Option<i32>) -> Job {
        Job {
            metadata: ObjectMeta {
                name: Some("nsscan-test-abc1234".to_string()),
                ..Default::default()
            },
            spec: Some(JobSpec {
                backoff_limit: backoff,
                template: PodTemplateSpec {
                    spec: Some(PodSpec {
                        init_containers: Some(vec![Container {
                            name: "init-pull".to_string(),
                            env: Some(vec![EnvVar {
                                name: "IMAGE_REF".to_string(),
                                value: Some("nginx:1.27.0".to_string()),
                                ..Default::default()
                            }]),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                ..Default::default()
            }),
            status: Some(JobStatus {
                succeeded,
                failed,
                ..Default::default()
            }),
        }
    }

    // -----------------------------------------------------------------------
    // T004 — parse_schedule valid cron
    // -----------------------------------------------------------------------

    #[test]
    fn t004_parse_schedule_valid_cron() {
        let result = parse_schedule(&schedule(Some("0 */6 * * *"), None));
        assert!(matches!(result, Ok(ScheduleSpec::Cron(_))));
    }

    // -----------------------------------------------------------------------
    // T005 — parse_schedule valid interval
    // -----------------------------------------------------------------------

    #[test]
    fn t005_parse_schedule_valid_interval() {
        let result = parse_schedule(&schedule(None, Some("6h")));
        match result {
            Ok(ScheduleSpec::Interval(d)) => {
                assert_eq!(d, Duration::from_secs(6 * 3600));
            }
            other => panic!("expected Interval(6h), got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // T006 — parse_schedule invalid cron
    // -----------------------------------------------------------------------

    #[test]
    fn t006_parse_schedule_invalid_cron_english_phrase() {
        let result = parse_schedule(&schedule(Some("every 6 hours"), None));
        assert!(matches!(result, Err(ScheduleError::InvalidCron(_))));
    }

    #[test]
    fn t006_parse_schedule_invalid_cron_empty() {
        let result = parse_schedule(&schedule(Some(""), None));
        assert!(matches!(result, Err(ScheduleError::InvalidCron(_))));
    }

    // -----------------------------------------------------------------------
    // T007 — parse_schedule invalid interval
    // -----------------------------------------------------------------------

    #[test]
    fn t007_parse_schedule_invalid_interval_garbage() {
        // humantime is liberal — accepts "6 hours", "6 h", etc. — so we use
        // a genuinely unparseable string to verify the InvalidInterval path.
        let result = parse_schedule(&schedule(None, Some("nonsense")));
        assert!(matches!(result, Err(ScheduleError::InvalidInterval(_))));
    }

    #[test]
    fn t007_parse_schedule_invalid_interval_negative_rejected() {
        let result = parse_schedule(&schedule(None, Some("-1h")));
        // humantime rejects negative durations as parse errors.
        assert!(matches!(result, Err(ScheduleError::InvalidInterval(_))));
    }

    #[test]
    fn t007_parse_schedule_invalid_interval_zero_is_below_minimum() {
        let result = parse_schedule(&schedule(None, Some("0s")));
        // Zero parses but is below the 60s minimum.
        assert!(matches!(
            result,
            Err(ScheduleError::IntervalBelowMinimum(_))
        ));
    }

    // -----------------------------------------------------------------------
    // T008 — both-set / neither-set
    // -----------------------------------------------------------------------

    #[test]
    fn t008_parse_schedule_both_set_is_error() {
        let result = parse_schedule(&schedule(Some("0 * * * *"), Some("1h")));
        assert_eq!(result.unwrap_err(), ScheduleError::BothSet);
    }

    #[test]
    fn t008_parse_schedule_neither_set_is_error() {
        let result = parse_schedule(&schedule(None, None));
        assert_eq!(result.unwrap_err(), ScheduleError::NeitherSet);
    }

    // -----------------------------------------------------------------------
    // T009 — parse_schedule interval below minimum
    // -----------------------------------------------------------------------

    #[test]
    fn t009_parse_schedule_interval_below_minimum_500ms() {
        let result = parse_schedule(&schedule(None, Some("500ms")));
        assert!(matches!(
            result,
            Err(ScheduleError::IntervalBelowMinimum(_))
        ));
    }

    #[test]
    fn t009_parse_schedule_interval_below_minimum_30s() {
        let result = parse_schedule(&schedule(None, Some("30s")));
        assert!(matches!(
            result,
            Err(ScheduleError::IntervalBelowMinimum(_))
        ));
    }

    #[test]
    fn t009_parse_schedule_interval_exact_minimum_passes() {
        let result = parse_schedule(&schedule(None, Some("60s")));
        assert!(matches!(result, Ok(ScheduleSpec::Interval(d)) if d == Duration::from_secs(60)));
    }

    // -----------------------------------------------------------------------
    // T010 — compute_next_scheduled_time cron
    // -----------------------------------------------------------------------

    #[test]
    fn t010_compute_next_cron_hourly() {
        // Anchor = 2026-06-29T14:00:00Z, cron = "0 * * * *" (hourly at minute 0),
        // jitter = 0. The next tick after 14:00:00 is 15:00:00.
        let s = parse_schedule(&schedule(Some("0 * * * *"), None)).unwrap();
        let next = compute_next_scheduled_time(&s, anchor(), Duration::from_secs(0));
        assert_eq!(next.to_rfc3339(), "2026-06-29T15:00:00+00:00");
    }

    #[test]
    fn t010_compute_next_cron_with_jitter() {
        let s = parse_schedule(&schedule(Some("0 * * * *"), None)).unwrap();
        let next = compute_next_scheduled_time(&s, anchor(), Duration::from_secs(17));
        assert_eq!(next.to_rfc3339(), "2026-06-29T15:00:17+00:00");
    }

    // -----------------------------------------------------------------------
    // T011 — compute_next_scheduled_time interval
    // -----------------------------------------------------------------------

    #[test]
    fn t011_compute_next_interval_one_hour() {
        let s = parse_schedule(&schedule(None, Some("1h"))).unwrap();
        let next = compute_next_scheduled_time(&s, anchor(), Duration::from_secs(15));
        assert_eq!(next.to_rfc3339(), "2026-06-29T15:00:15+00:00");
    }

    #[test]
    fn t011_compute_next_interval_no_jitter() {
        let s = parse_schedule(&schedule(None, Some("2m"))).unwrap();
        let next = compute_next_scheduled_time(&s, anchor(), Duration::from_secs(0));
        assert_eq!(next.to_rfc3339(), "2026-06-29T14:02:00+00:00");
    }

    // -----------------------------------------------------------------------
    // T012 — cr_uid_jitter_seconds
    // -----------------------------------------------------------------------

    #[test]
    fn t012_jitter_deterministic_for_same_uid() {
        let uid = "00000000-0000-0000-0000-000000000007";
        let a = cr_uid_jitter_seconds(uid);
        let b = cr_uid_jitter_seconds(uid);
        assert_eq!(a, b, "same uid MUST produce same jitter across calls");
    }

    #[test]
    fn t012_jitter_bounded_to_0_60() {
        // Sample a handful of UIDs.
        for n in 0..100 {
            let uid = format!("uid-{n:04}");
            let j = cr_uid_jitter_seconds(&uid);
            assert!(j < 60, "jitter {j} out of bounds for uid={uid}");
        }
    }

    #[test]
    fn t012_jitter_distinct_uids_produce_distinct_outputs() {
        // Strict distinctness across 100 UIDs is too tight (60 buckets, pigeon-
        // hole guarantees collisions). Assert that the SET of jitter values
        // covers >= 30 distinct values across 100 UIDs (uniform-ish spread).
        let mut seen = std::collections::HashSet::new();
        for n in 0..100 {
            seen.insert(cr_uid_jitter_seconds(&format!("uid-{n}")));
        }
        assert!(
            seen.len() >= 30,
            "expected >= 30 distinct jitter buckets across 100 UIDs, got {}",
            seen.len()
        );
    }

    // -----------------------------------------------------------------------
    // T013 — is_schedule_due
    // -----------------------------------------------------------------------

    #[test]
    fn t013_is_schedule_due_true_when_now_at_or_past_next() {
        let next = anchor();
        assert!(is_schedule_due(next, next));
        assert!(is_schedule_due(next, next + chrono::Duration::seconds(1)));
        assert!(is_schedule_due(next, next + chrono::Duration::hours(1)));
    }

    #[test]
    fn t013_is_schedule_due_false_when_now_before_next() {
        let next = anchor();
        assert!(!is_schedule_due(next, next - chrono::Duration::seconds(1)));
        assert!(!is_schedule_due(next, next - chrono::Duration::hours(1)));
    }

    // -----------------------------------------------------------------------
    // T014 — should_delete_job filter correctness (FR-011)
    // -----------------------------------------------------------------------

    #[test]
    fn t014_should_delete_succeeded_job() {
        let job = job_with_status(Some(1), None, Some(6));
        assert!(should_delete_job(&job));
    }

    #[test]
    fn t014_should_delete_finally_failed_job() {
        let job = job_with_status(None, Some(7), Some(6));
        assert!(should_delete_job(&job));
    }

    #[test]
    fn t014_should_not_delete_in_progress_job() {
        // succeeded=0, failed < backoffLimit+1
        let job = job_with_status(None, Some(3), Some(6));
        assert!(!should_delete_job(&job));
    }

    #[test]
    fn t014_should_not_delete_pending_job() {
        // succeeded=None, failed=None (just-created Job not yet observed)
        let job = job_with_status(None, None, Some(6));
        assert!(!should_delete_job(&job));
    }

    // -----------------------------------------------------------------------
    // T033 — single-decision per reconcile (no catch-up iteration)
    // -----------------------------------------------------------------------

    #[test]
    fn t033_compute_next_with_past_anchor_returns_one_future_time() {
        // Anchor = 5 minutes ago. Interval = 60s. The next scheduled time is
        // anchor + 60s = 4 minutes ago — still in the past. is_schedule_due
        // returns true exactly once for "now", regardless of how far past
        // the anchor was. No iteration to "catch up" past windows.
        let now = Utc::now();
        let past_anchor = now - chrono::Duration::minutes(5);
        let s = parse_schedule(&schedule(None, Some("60s"))).unwrap();
        let next = compute_next_scheduled_time(&s, past_anchor, Duration::from_secs(0));
        // next is past_anchor + 60s = (-5min + 1min) = -4min — in the past.
        assert!(
            next < now,
            "next should be past for an old anchor + 60s interval"
        );
        assert!(is_schedule_due(next, now));
        // Single boolean — no iteration over missed windows. FR-008 satisfied
        // by construction.
    }
}
