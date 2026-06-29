//! Reconciler for `NamespaceScan` CRs.
//!
//! Per spec FRs 003/004/010/011/012 (feature 002): every CR gets a `Ready=False`
//! condition and a refreshed `lastReconciledAt` on every reconcile cycle.
//! Idempotency and `InvalidSpec` rules are computed by
//! [`crate::status::desired_status`].
//!
//! Feature 007 extends this loop: for valid specs, the reconciler invokes
//! [`crate::reconcile::scan_orchestrator::ensure_jobs`] to enumerate target
//! pods, dedupe their container images, and idempotently create one
//! `batch/v1.Job` per distinct in-scope image. The orchestration result drives
//! the new status reasons (`Scanning`, `NoImagesInScope`, `BuildFailed`,
//! `RBACInsufficient`) via
//! [`crate::status::status_with_orchestration_result`].

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use kube::{
    api::{Patch, PatchParams},
    runtime::controller::Action,
    Api, Client, ResourceExt,
};
use serde_json::json;
use thiserror::Error;
use tracing::{error, info};

use crate::crds::namespace_scan::NamespaceScan;
use crate::reconcile::scan_orchestrator::{ensure_jobs, CrMetaSnapshot, OrchestrationResult};
use crate::reconcile::scheduler::{
    self, cleanup_terminal_jobs, compute_next_scheduled_time, cr_uid_jitter_seconds,
    is_schedule_due,
};
use crate::reconcile::status_aggregator::{aggregate_job_outcomes, list_owned_jobs};
use crate::status::{
    desired_status, status_with_aggregated_outcome, status_with_orchestration_result,
    REASON_INVALID_SPEC, REASON_SCAN_COMPLETED, REASON_SCAN_FAILED,
};

const REQUEUE_INTERVAL: Duration = Duration::from_secs(300);
const ERROR_REQUEUE: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
    /// Namespace in which the operator pod runs. Spawned scan Jobs land here
    /// (feature 007 FR-003). Populated from `POD_NAMESPACE` in `main.rs`.
    pub operator_namespace: String,
}

#[derive(Debug, Error)]
pub enum ReconcileError {
    #[error("kube API error: {0}")]
    Kube(#[from] kube::Error),
}

impl From<crate::reconcile::scan_orchestrator::OrchestrationError> for ReconcileError {
    fn from(err: crate::reconcile::scan_orchestrator::OrchestrationError) -> Self {
        match err {
            crate::reconcile::scan_orchestrator::OrchestrationError::Kube(k) => {
                ReconcileError::Kube(k)
            }
        }
    }
}

pub async fn reconcile(obj: Arc<NamespaceScan>, ctx: Arc<Ctx>) -> Result<Action, ReconcileError> {
    let now = Utc::now();
    let name = obj.name_any();
    let namespace = obj.namespace().unwrap_or_default();

    let api: Api<NamespaceScan> = if namespace.is_empty() {
        Api::all(ctx.client.clone())
    } else {
        Api::namespaced(ctx.client.clone(), &namespace)
    };

    let base_status = desired_status(&obj.spec, now, obj.status.as_ref());

    let base_reason = base_status
        .conditions
        .first()
        .and_then(|c| c.reason.as_deref())
        .unwrap_or("unknown")
        .to_string();

    // For invalid specs (target missing both namespaces + labelSelector),
    // feature 002's `desired_status` already returned `InvalidSpec`. Skip the
    // orchestrator entirely — there's nothing to enumerate.
    let new_status = if base_reason == REASON_INVALID_SPEC {
        base_status
    } else {
        // Feature 007: invoke the orchestrator. CR's `uid` is required for
        // the spawned Jobs' owner-references; if missing (extremely unusual —
        // the API server stamps it on create), fall back to base status.
        let uid = obj.metadata.uid.clone().unwrap_or_default();
        if uid.is_empty() {
            error!(
                event = "reconcile_missing_uid",
                namespace_scan = %name,
                "CR has no metadata.uid; skipping Job orchestration (this should not happen)",
            );
            base_status
        } else {
            let cr_meta = CrMetaSnapshot {
                name: name.clone(),
                uid,
                namespace: namespace.clone(),
            };
            let orchestration = ensure_jobs(&obj.spec, &cr_meta, &ctx).await?;
            info!(
                event = "scan_orchestration_result",
                namespace_scan = %name,
                namespace = %namespace,
                result = ?orchestration,
                "scan orchestration completed",
            );
            let after_feat_007 = status_with_orchestration_result(
                base_status,
                obj.status.as_ref(),
                &orchestration,
                now,
            );
            // Feature 008: aggregate owned Jobs and overlay ScanCompleted /
            // ScanFailed onto the Scanning base. FR-009 gate: this branch ONLY
            // runs when feature 007 returned Spawned; other variants
            // (NoImagesInScope, BuildFailed, RbacInsufficient) keep feature
            // 007's status verbatim.
            if let OrchestrationResult::Spawned { .. } = &orchestration {
                let jobs_api: Api<k8s_openapi::api::batch::v1::Job> =
                    Api::namespaced(ctx.client.clone(), &ctx.operator_namespace);
                let owned = list_owned_jobs(&jobs_api, &name).await?;
                let outcome = aggregate_job_outcomes(&owned, &obj.spec);
                info!(
                    event = "scan_aggregation_result",
                    namespace_scan = %name,
                    job_count = owned.len(),
                    outcome = ?outcome,
                    "job-status aggregation completed",
                );
                status_with_aggregated_outcome(after_feat_007, obj.status.as_ref(), &outcome, now)
            } else {
                after_feat_007
            }
        }
    };

    let final_reason = new_status
        .conditions
        .first()
        .and_then(|c| c.reason.as_deref())
        .unwrap_or("unknown")
        .to_string();

    // Feature 009: schedule honoring. Only attempt the schedule path when the
    // reason is in a terminal scan state (ScanCompleted or ScanFailed) — gates
    // FR-006 (no re-scan while previous is in progress) and FR-009 (don't
    // override NoImagesInScope/BuildFailed/RBACInsufficient). The structural
    // gate is verified by T025c's grep check.
    let mut new_status = new_status;
    let mut schedule_due_action: Option<Duration> = None;
    if final_reason == REASON_SCAN_COMPLETED || final_reason == REASON_SCAN_FAILED {
        let (next_scheduled_opt, due) = schedule_decision(&obj, &new_status, now);
        if let Some(next_scheduled) = next_scheduled_opt {
            new_status.next_scheduled_scan_at = Some(next_scheduled.to_rfc3339());

            // (f) Emit structured `schedule_decision` log regardless of fire/defer
            // (FR-014, paired with T036's grep check).
            info!(
                event = "schedule_decision",
                namespace_scan = %name,
                namespace = %namespace,
                last_scan_completed_at = ?new_status.last_scan_completed_at,
                next_scheduled = %next_scheduled.to_rfc3339(),
                decision = if due { "fire" } else { "defer" },
                "schedule decision computed",
            );

            if due {
                let jobs_api: Api<k8s_openapi::api::batch::v1::Job> =
                    Api::namespaced(ctx.client.clone(), &ctx.operator_namespace);
                let owned = list_owned_jobs(&jobs_api, &name).await?;
                let deleted = cleanup_terminal_jobs(&jobs_api, &owned).await?;
                info!(
                    event = "schedule_rescan_cleanup",
                    namespace_scan = %name,
                    deleted_jobs = deleted,
                    "deleted terminal Jobs for scheduled re-scan",
                );
                // Short requeue so the next reconcile picks up the empty-Job-list
                // state and ensure_jobs respawns fresh.
                schedule_due_action = Some(Duration::from_secs(5));
            }
        }
    }

    info!(
        event = "reconcile",
        namespace_scan = %name,
        namespace = %namespace,
        reason = %final_reason,
        "reconciled NamespaceScan",
    );

    let patch = json!({
        "status": new_status,
    });
    api.patch_status(&name, &PatchParams::default(), &Patch::Merge(&patch))
        .await?;

    // Feature 009 requeue cadence (T026): if a schedule fire just happened,
    // short-requeue. Else if the next scheduled time is within 1m, requeue at
    // that moment + 1s. Else keep the existing 5m heartbeat.
    let requeue = if let Some(d) = schedule_due_action {
        d
    } else {
        next_requeue_interval(&new_status, now)
    };
    Ok(Action::requeue(requeue))
}

/// Resolve the scheduling decision for this reconcile pass: compute the next
/// scheduled time (if the schedule parses) and whether it's due. Returns
/// `(None, false)` when the schedule is invalid (caller's `desired_status`
/// already surfaced this as InvalidSpec on a prior cycle) — but feature 009's
/// FR-009 gate means this function is only called when the reason is a
/// terminal scan state, which implies the schedule already parsed at least
/// once.
fn schedule_decision(
    obj: &NamespaceScan,
    new_status: &crate::crds::namespace_scan::NamespaceScanStatus,
    now: chrono::DateTime<chrono::Utc>,
) -> (Option<chrono::DateTime<chrono::Utc>>, bool) {
    let schedule_spec = match scheduler::parse_schedule(&obj.spec.schedule) {
        Ok(s) => s,
        Err(_) => return (None, false),
    };

    // Resolve the anchor: lastScanCompletedAt → creationTimestamp → now.
    let anchor = new_status
        .last_scan_completed_at
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .or_else(|| obj.metadata.creation_timestamp.as_ref().map(|t| t.0))
        .unwrap_or(now);

    let jitter = std::time::Duration::from_secs(cr_uid_jitter_seconds(
        obj.metadata.uid.as_deref().unwrap_or(""),
    ));
    let next_scheduled = compute_next_scheduled_time(&schedule_spec, anchor, jitter);
    let due = is_schedule_due(next_scheduled, now);
    (Some(next_scheduled), due)
}

/// Compute the next reconcile requeue interval. When the next scheduled scan
/// is imminent (< 1 minute), requeue tightly to fire just after; otherwise
/// keep feature 002's 5-minute heartbeat. T026 — fulfills SC-001/SC-002's
/// 30-second precision budget.
fn next_requeue_interval(
    new_status: &crate::crds::namespace_scan::NamespaceScanStatus,
    now: chrono::DateTime<chrono::Utc>,
) -> Duration {
    let Some(next_str) = new_status.next_scheduled_scan_at.as_deref() else {
        return REQUEUE_INTERVAL;
    };
    let Ok(next) = chrono::DateTime::parse_from_rfc3339(next_str) else {
        return REQUEUE_INTERVAL;
    };
    let next = next.with_timezone(&chrono::Utc);
    let until_next = next.signed_duration_since(now);
    if until_next > chrono::Duration::zero() && until_next < chrono::Duration::minutes(1) {
        // Requeue just after the scheduled tick.
        until_next.to_std().unwrap_or(REQUEUE_INTERVAL) + Duration::from_secs(1)
    } else {
        REQUEUE_INTERVAL
    }
}

pub fn error_policy(obj: Arc<NamespaceScan>, err: &ReconcileError, _ctx: Arc<Ctx>) -> Action {
    let name = obj.name_any();

    // NotFound is benign: the CR was deleted while reconcile was in flight.
    if let ReconcileError::Kube(kube::Error::Api(api_err)) = err {
        if api_err.code == 404 {
            info!(
                event = "reconcile_cr_deleted",
                namespace_scan = %name,
                "CR was deleted mid-reconcile; nothing to do",
            );
            return Action::await_change();
        }
    }

    error!(
        event = "reconcile_error",
        namespace_scan = %name,
        error = %format!("{err:#}"),
        "reconcile failed; requeueing with backoff",
    );
    Action::requeue(ERROR_REQUEUE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crds::namespace_scan::{
        NamespaceScanSpec, NamespaceScanStatus, Output, OutputType, ScanFormat, Schedule,
        StatusCondition, Target,
    };
    use crate::status::{
        READY, REASON_SCAN_COMPLETED, REASON_SCAN_FAILED, STATUS_FALSE, STATUS_TRUE,
    };
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;

    fn valid_spec_with_schedule(cron: Option<&str>, interval: Option<&str>) -> NamespaceScanSpec {
        NamespaceScanSpec {
            target: Target {
                namespaces: vec!["default".to_string()],
                kinds: vec![],
                label_selector: None,
            },
            schedule: Schedule {
                cron: cron.map(String::from),
                interval: interval.map(String::from),
            },
            mikebom_image: "ghcr.io/kusari-oss/mikebom:test".to_string(),
            scan_format: ScanFormat::CyclonedxJson,
            output: Output {
                backend_type: OutputType::Pvc,
                pvc: None,
                s3: None,
                oci: None,
            },
        }
    }

    fn make_cr(
        cron: Option<&str>,
        interval: Option<&str>,
        last_scan_completed_at: Option<&str>,
    ) -> NamespaceScan {
        let mut cr = NamespaceScan::new("scan-test", valid_spec_with_schedule(cron, interval));
        cr.metadata.uid = Some("11111111-2222-3333-4444-555555555555".to_string());
        cr.metadata.namespace = Some("kusari-operator".to_string());
        cr.metadata.creation_timestamp =
            Some(Time(chrono::Utc::now() - chrono::Duration::hours(1)));
        cr.status = Some(NamespaceScanStatus {
            conditions: vec![],
            last_reconciled_at: None,
            last_scan_completed_at: last_scan_completed_at.map(String::from),
            scanned_images: vec![],
            next_scheduled_scan_at: None,
        });
        cr
    }

    fn scan_completed_status() -> NamespaceScanStatus {
        NamespaceScanStatus {
            conditions: vec![StatusCondition {
                condition_type: READY.to_string(),
                status: STATUS_TRUE.to_string(),
                reason: Some(REASON_SCAN_COMPLETED.to_string()),
                message: Some("scanned 1 distinct image successfully".to_string()),
                last_transition_time: Some("2026-06-29T14:00:30Z".to_string()),
            }],
            last_reconciled_at: Some("2026-06-29T14:00:30Z".to_string()),
            last_scan_completed_at: Some("2026-06-29T14:00:30Z".to_string()),
            scanned_images: vec![],
            next_scheduled_scan_at: None,
        }
    }

    // -----------------------------------------------------------------------
    // T025b — nextScheduledScanAt is populated and future-relative-to-anchor
    // (FR-010 + SC-007)
    // -----------------------------------------------------------------------

    #[test]
    fn t025b_schedule_decision_populates_future_next_scheduled() {
        let cr = make_cr(None, Some("1h"), Some("2026-06-29T14:00:30Z"));
        let status = scan_completed_status();
        let now = chrono::DateTime::parse_from_rfc3339("2026-06-29T14:30:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        let (next, due) = schedule_decision(&cr, &status, now);
        let next = next.expect("schedule_decision MUST return Some for a valid schedule");

        // anchor = 14:00:30, interval = 1h, jitter <= 59s → next ∈ (15:00:30, 15:01:29]
        let expected_min = chrono::DateTime::parse_from_rfc3339("2026-06-29T15:00:30Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let expected_max = chrono::DateTime::parse_from_rfc3339("2026-06-29T15:01:30Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert!(
            next >= expected_min && next <= expected_max,
            "next ({next}) MUST be in (15:00:30, 15:01:30]"
        );
        // At 14:30:00, next (~15:00:30) has NOT elapsed yet.
        assert!(
            !due,
            "schedule MUST NOT be due 30min after last scan when interval=1h"
        );
    }

    #[test]
    fn t025b_schedule_decision_returns_none_for_invalid_schedule() {
        // Both-set is invalid. desired_status would have caught this on a
        // prior cycle, but schedule_decision still must handle it gracefully.
        let cr = make_cr(Some("0 * * * *"), Some("1h"), Some("2026-06-29T14:00:30Z"));
        let status = scan_completed_status();
        let now = chrono::Utc::now();
        let (next, due) = schedule_decision(&cr, &status, now);
        assert!(next.is_none(), "invalid schedule MUST return None");
        assert!(!due);
    }

    #[test]
    fn t025b_schedule_decision_anchor_fallback_to_creation_when_last_scan_unset() {
        let cr = make_cr(None, Some("1h"), None);
        // creation_timestamp is set by make_cr to 1h ago.
        let creation = cr.metadata.creation_timestamp.as_ref().unwrap().0;
        // Must clear last_scan_completed_at on the status to exercise the fallback.
        let mut status = scan_completed_status();
        status.last_scan_completed_at = None;
        let now = creation + chrono::Duration::minutes(30); // 30min after creation

        let (next, due) = schedule_decision(&cr, &status, now);
        let next = next.expect("schedule_decision MUST return Some");
        // anchor falls back to creation_timestamp; with interval=1h, next ≈ creation + 1h + jitter (≤ 59s).
        let expected_min = creation + chrono::Duration::minutes(60);
        let expected_max = creation + chrono::Duration::minutes(60) + chrono::Duration::seconds(60);
        assert!(
            next >= expected_min && next <= expected_max,
            "anchor MUST fall back to creation_timestamp; got next={next}, expected in [{expected_min}, {expected_max}]"
        );
        assert!(!due);

        // Sanity: set last_scan_completed_at to override the fallback. With anchor
        // = now (= creation+30min), next2 ≈ creation + 90min + jitter — distinct
        // from the fallback-derived `next` (which was ~creation+60min).
        status.last_scan_completed_at = Some(now.to_rfc3339());
        let (next2, _) = schedule_decision(&cr, &status, now);
        let next2 = next2.unwrap();
        assert!(
            next2 != next,
            "different anchor MUST yield different next-scheduled time; next={next}, next2={next2}"
        );
    }

    // -----------------------------------------------------------------------
    // T026 — requeue cadence tightening
    // -----------------------------------------------------------------------

    #[test]
    fn t026_requeue_short_when_next_scheduled_is_imminent() {
        let now = chrono::Utc::now();
        let mut status = scan_completed_status();
        status.next_scheduled_scan_at = Some((now + chrono::Duration::seconds(30)).to_rfc3339());
        let r = next_requeue_interval(&status, now);
        // Should be ~31s (30s remaining + 1s buffer).
        assert!(
            r >= Duration::from_secs(30) && r <= Duration::from_secs(32),
            "requeue MUST be tight when next-scheduled is imminent; got {r:?}"
        );
    }

    #[test]
    fn t026_requeue_heartbeat_when_next_scheduled_is_far() {
        let now = chrono::Utc::now();
        let mut status = scan_completed_status();
        status.next_scheduled_scan_at = Some((now + chrono::Duration::hours(6)).to_rfc3339());
        let r = next_requeue_interval(&status, now);
        assert_eq!(
            r, REQUEUE_INTERVAL,
            "requeue MUST be 5m heartbeat when next-scheduled is hours away"
        );
    }

    #[test]
    fn t026_requeue_heartbeat_when_no_next_scheduled() {
        let now = chrono::Utc::now();
        let status = scan_completed_status(); // next_scheduled_scan_at = None
        let r = next_requeue_interval(&status, now);
        assert_eq!(r, REQUEUE_INTERVAL);
    }

    // -----------------------------------------------------------------------
    // T025c gate guard — silently document the structural gate exists.
    // The actual grep verification (T025c) is in Phase 6 against this file.
    // -----------------------------------------------------------------------

    #[test]
    fn fr_006_gate_constants_referenced_in_module() {
        // This test exists purely to import REASON_SCAN_COMPLETED / REASON_SCAN_FAILED
        // at the test boundary, ensuring they're not accidentally renamed.
        // The reconcile() function uses both for the FR-006 in-progress gate.
        assert_eq!(REASON_SCAN_COMPLETED, "ScanCompleted");
        assert_eq!(REASON_SCAN_FAILED, "ScanFailed");
        // STATUS_FALSE referenced to confirm the imports are live.
        assert_eq!(STATUS_FALSE, "False");
    }
}
