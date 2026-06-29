//! Status condition + timestamp computation for `NamespaceScan`.
//!
//! Pure function `desired_status` is the single source of truth for what the
//! reconciler should patch onto a CR. Tests live in the same file so the shape
//! decisions are exercised every `cargo test --workspace` run.
//!
//! See `specs/002-reconciler-skeleton/contracts/namespacescan-status.md` for
//! the user-visible contract this module implements.

use chrono::{DateTime, Utc};

use crate::crds::namespace_scan::{
    NamespaceScanSpec, NamespaceScanStatus, StatusCondition, Target,
};

pub const READY: &str = "Ready";
pub const STATUS_FALSE: &str = "False";
pub const REASON_NOT_YET_RECONCILED: &str = "NotYetReconciled";
pub const REASON_INVALID_SPEC: &str = "InvalidSpec";
// Feature 007 reasons — written when the orchestrator runs against a valid spec.
// All four are `Ready=False`; feature 008 will introduce the first `Ready=True`
// transition via `ScanCompleted`.
pub const REASON_SCANNING: &str = "Scanning";
pub const REASON_NO_IMAGES_IN_SCOPE: &str = "NoImagesInScope";
pub const REASON_RBAC_INSUFFICIENT: &str = "RBACInsufficient";
pub const REASON_BUILD_FAILED: &str = "BuildFailed";

const MESSAGE_NOT_YET_RECONCILED: &str =
    "Scanning not yet implemented; mikebom-operator feature 003 introduces the Job spec.";
const MESSAGE_INVALID_SPEC: &str =
    "spec.target requires either namespaces or labelSelector to be non-empty";

/// Compute the desired status for a `NamespaceScan` given its current spec and
/// the existing status (if any). Pure function — no I/O, no side effects.
///
/// Behavior:
/// * Always emits exactly one `Ready=False` condition.
/// * `lastReconciledAt` advances every call.
/// * `lastTransitionTime` is preserved when the condition's `(status, reason)`
///   pair is unchanged; otherwise it advances to `now`.
/// * `lastScanCompletedAt` and `scannedImages` are passed through unchanged from
///   the existing status (this feature doesn't touch them).
pub fn desired_status(
    spec: &NamespaceScanSpec,
    now: DateTime<Utc>,
    existing: Option<&NamespaceScanStatus>,
) -> NamespaceScanStatus {
    let (reason, message) = if is_target_valid(&spec.target) {
        (REASON_NOT_YET_RECONCILED, MESSAGE_NOT_YET_RECONCILED)
    } else {
        (REASON_INVALID_SPEC, MESSAGE_INVALID_SPEC)
    };

    let last_transition_time = existing
        .and_then(|s| s.conditions.iter().find(|c| c.condition_type == READY))
        .and_then(|c| {
            if c.status == STATUS_FALSE && c.reason.as_deref() == Some(reason) {
                c.last_transition_time.clone()
            } else {
                None
            }
        })
        .unwrap_or_else(|| now.to_rfc3339());

    let condition = StatusCondition {
        condition_type: READY.to_string(),
        status: STATUS_FALSE.to_string(),
        reason: Some(reason.to_string()),
        message: Some(message.to_string()),
        last_transition_time: Some(last_transition_time),
    };

    NamespaceScanStatus {
        conditions: vec![condition],
        last_reconciled_at: Some(now.to_rfc3339()),
        last_scan_completed_at: existing.and_then(|s| s.last_scan_completed_at.clone()),
        scanned_images: existing
            .map(|s| s.scanned_images.clone())
            .unwrap_or_default(),
    }
}

/// Apply the result of `scan_orchestrator::ensure_jobs` to a base status,
/// translating the orchestration outcome into the appropriate `Ready=False`
/// reason per the decision table in research.md §8.
///
/// Caller MUST NOT invoke this with a base whose reason is already
/// `InvalidSpec` — feature 002 short-circuits before orchestration runs.
/// `lastReconciledAt` and `scannedImages` pass through unchanged.
pub fn status_with_orchestration_result(
    base: NamespaceScanStatus,
    existing: Option<&NamespaceScanStatus>,
    result: &crate::reconcile::scan_orchestrator::OrchestrationResult,
    now: DateTime<Utc>,
) -> NamespaceScanStatus {
    use crate::reconcile::scan_orchestrator::OrchestrationResult;

    let (reason, message) = match result {
        OrchestrationResult::Spawned { distinct_images } => (
            REASON_SCANNING,
            format!(
                "scanning {distinct_images} distinct image{plural} across target namespaces",
                plural = if *distinct_images == 1 { "" } else { "s" }
            ),
        ),
        OrchestrationResult::NoImagesInScope => (
            REASON_NO_IMAGES_IN_SCOPE,
            "target resolved to zero pods in phase Running or Pending".to_string(),
        ),
        OrchestrationResult::BuildFailed { image_ref, error } => (
            REASON_BUILD_FAILED,
            format!("failed to build scan Job for image {image_ref:?}: {error}"),
        ),
        OrchestrationResult::RbacInsufficient {
            verb_resource,
            namespace,
            message,
        } => (
            REASON_RBAC_INSUFFICIENT,
            match namespace {
                Some(ns) => {
                    format!("operator lacks RBAC to {verb_resource} in namespace {ns}: {message}")
                }
                None => format!("operator lacks RBAC to {verb_resource}: {message}"),
            },
        ),
    };

    // Preserve lastTransitionTime if (status, reason) is unchanged — matches
    // feature 002's idempotency rule. Looked up on `existing` (the prior
    // patched status), not on `base` (which always reflects either
    // NotYetReconciled or InvalidSpec right after desired_status runs).
    let last_transition_time = existing
        .and_then(|s| s.conditions.iter().find(|c| c.condition_type == READY))
        .and_then(|c| {
            if c.status == STATUS_FALSE && c.reason.as_deref() == Some(reason) {
                c.last_transition_time.clone()
            } else {
                None
            }
        })
        .unwrap_or_else(|| now.to_rfc3339());

    let condition = StatusCondition {
        condition_type: READY.to_string(),
        status: STATUS_FALSE.to_string(),
        reason: Some(reason.to_string()),
        message: Some(message),
        last_transition_time: Some(last_transition_time),
    };

    NamespaceScanStatus {
        conditions: vec![condition],
        last_reconciled_at: base.last_reconciled_at,
        last_scan_completed_at: base.last_scan_completed_at,
        scanned_images: base.scanned_images,
    }
}

fn is_target_valid(target: &Target) -> bool {
    if !target.namespaces.is_empty() {
        return true;
    }
    if let Some(sel) = target.label_selector.as_deref() {
        if !sel.trim().is_empty() {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crds::namespace_scan::{
        NamespaceScanSpec, Output, OutputType, ScanFormat, Schedule, Target,
    };
    use chrono::Duration as ChronoDuration;

    fn valid_spec() -> NamespaceScanSpec {
        NamespaceScanSpec {
            target: Target {
                namespaces: vec!["default".to_string()],
                kinds: vec![],
                label_selector: None,
            },
            schedule: Schedule {
                cron: Some("0 */6 * * *".to_string()),
                interval: None,
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

    fn invalid_spec() -> NamespaceScanSpec {
        let mut s = valid_spec();
        s.target.namespaces = vec![];
        s.target.label_selector = None;
        s
    }

    #[test]
    fn valid_spec_sets_not_yet_reconciled() {
        let now = Utc::now();
        let status = desired_status(&valid_spec(), now, None);

        assert_eq!(status.conditions.len(), 1);
        let c = &status.conditions[0];
        assert_eq!(c.condition_type, READY);
        assert_eq!(c.status, STATUS_FALSE);
        assert_eq!(c.reason.as_deref(), Some(REASON_NOT_YET_RECONCILED));
        assert!(c.message.as_deref().unwrap().contains("feature 003"));
        assert_eq!(status.last_reconciled_at, Some(now.to_rfc3339()));
    }

    #[test]
    fn invalid_spec_sets_invalid_spec_reason() {
        let now = Utc::now();
        let status = desired_status(&invalid_spec(), now, None);

        let c = &status.conditions[0];
        assert_eq!(c.reason.as_deref(), Some(REASON_INVALID_SPEC));
    }

    #[test]
    fn empty_label_selector_string_still_invalid() {
        let mut spec = invalid_spec();
        spec.target.label_selector = Some("   ".to_string());
        let status = desired_status(&spec, Utc::now(), None);
        assert_eq!(
            status.conditions[0].reason.as_deref(),
            Some(REASON_INVALID_SPEC),
        );
    }

    #[test]
    fn idempotent_preserves_last_transition_time() {
        let earlier = Utc::now() - ChronoDuration::seconds(60);
        let later = Utc::now();

        let first = desired_status(&valid_spec(), earlier, None);
        let second = desired_status(&valid_spec(), later, Some(&first));

        // Same (status, reason) — lastTransitionTime is preserved.
        assert_eq!(
            first.conditions[0].last_transition_time, second.conditions[0].last_transition_time,
            "lastTransitionTime should not advance when reconciling unchanged spec",
        );
        // lastReconciledAt always refreshes.
        assert_ne!(first.last_reconciled_at, second.last_reconciled_at);
        assert_eq!(second.last_reconciled_at, Some(later.to_rfc3339()));
    }

    #[test]
    fn transition_updates_last_transition_time() {
        let earlier = Utc::now() - ChronoDuration::seconds(60);
        let later = Utc::now();

        let first = desired_status(&valid_spec(), earlier, None);
        let second = desired_status(&invalid_spec(), later, Some(&first));

        // Reason changed: NotYetReconciled → InvalidSpec.
        assert_ne!(first.conditions[0].reason, second.conditions[0].reason);
        assert_eq!(
            second.conditions[0].last_transition_time,
            Some(later.to_rfc3339()),
            "lastTransitionTime should advance when reason changes",
        );
    }

    #[test]
    fn preserves_unrelated_status_fields() {
        let now = Utc::now();
        let mut existing = desired_status(&valid_spec(), now, None);
        existing.last_scan_completed_at = Some("2026-01-01T00:00:00Z".to_string());

        let later = desired_status(&valid_spec(), Utc::now(), Some(&existing));
        assert_eq!(
            later.last_scan_completed_at.as_deref(),
            Some("2026-01-01T00:00:00Z"),
            "lastScanCompletedAt should pass through (we don't manage it in this feature)",
        );
    }

    // -----------------------------------------------------------------------
    // T022 — status_with_orchestration_result mapping table
    // -----------------------------------------------------------------------

    use crate::reconcile::scan_orchestrator::OrchestrationResult;

    #[test]
    fn t022_spawned_maps_to_scanning_with_distinct_image_count_in_message() {
        let now = Utc::now();
        let base = desired_status(&valid_spec(), now, None);
        let result = OrchestrationResult::Spawned { distinct_images: 3 };
        let out = status_with_orchestration_result(base, None, &result, now);

        assert_eq!(out.conditions.len(), 1);
        let cond = &out.conditions[0];
        assert_eq!(cond.condition_type, READY);
        assert_eq!(cond.status, STATUS_FALSE);
        assert_eq!(cond.reason.as_deref(), Some(REASON_SCANNING));
        let msg = cond.message.as_deref().unwrap_or("");
        assert!(
            msg.contains('3') && msg.contains("images"),
            "message should mention image count + 'images': {msg}",
        );
    }

    #[test]
    fn t022_spawned_singular_message_for_one_image() {
        let now = Utc::now();
        let base = desired_status(&valid_spec(), now, None);
        let result = OrchestrationResult::Spawned { distinct_images: 1 };
        let out = status_with_orchestration_result(base, None, &result, now);
        let msg = out.conditions[0].message.as_deref().unwrap_or("");
        assert!(
            msg.contains("1 distinct image ")
                || msg.ends_with("1 distinct image across target namespaces"),
            "singular wording expected, got: {msg}",
        );
    }

    #[test]
    fn t022_no_images_in_scope_maps_to_distinct_reason() {
        let now = Utc::now();
        let base = desired_status(&valid_spec(), now, None);
        let result = OrchestrationResult::NoImagesInScope;
        let out = status_with_orchestration_result(base, None, &result, now);
        assert_eq!(
            out.conditions[0].reason.as_deref(),
            Some(REASON_NO_IMAGES_IN_SCOPE),
        );
        assert!(out.conditions[0]
            .message
            .as_deref()
            .unwrap_or("")
            .contains("Running"));
    }

    #[test]
    fn t022_build_failed_message_names_image_and_error() {
        let now = Utc::now();
        let base = desired_status(&valid_spec(), now, None);
        let result = OrchestrationResult::BuildFailed {
            image_ref: "ghcr.io/example:bad".to_string(),
            error: "spec.output.type=Pvc requires spec.output.pvc.claimName to be non-empty"
                .to_string(),
        };
        let out = status_with_orchestration_result(base, None, &result, now);
        assert_eq!(
            out.conditions[0].reason.as_deref(),
            Some(REASON_BUILD_FAILED)
        );
        let msg = out.conditions[0].message.as_deref().unwrap_or("");
        assert!(
            msg.contains("ghcr.io/example:bad") && msg.contains("claimName"),
            "message must name failing image + builder error verbatim: {msg}",
        );
    }

    #[test]
    fn t022_rbac_insufficient_message_names_verb_and_namespace() {
        let now = Utc::now();
        let base = desired_status(&valid_spec(), now, None);
        let result = OrchestrationResult::RbacInsufficient {
            verb_resource: "list pods".to_string(),
            namespace: Some("prod".to_string()),
            message: "pods is forbidden: User cannot list resource".to_string(),
        };
        let out = status_with_orchestration_result(base, None, &result, now);
        assert_eq!(
            out.conditions[0].reason.as_deref(),
            Some(REASON_RBAC_INSUFFICIENT),
        );
        let msg = out.conditions[0].message.as_deref().unwrap_or("");
        assert!(
            msg.contains("list pods") && msg.contains("prod"),
            "message must name verb+namespace+kube error: {msg}",
        );
        assert!(
            msg.contains("pods is forbidden"),
            "message must include the kube ErrorResponse.message verbatim: {msg}",
        );
    }

    #[test]
    fn t022_rbac_insufficient_no_namespace_falls_back_to_verb_only() {
        let now = Utc::now();
        let base = desired_status(&valid_spec(), now, None);
        let result = OrchestrationResult::RbacInsufficient {
            verb_resource: "create batch/v1.jobs".to_string(),
            namespace: None,
            message: "jobs.batch is forbidden".to_string(),
        };
        let out = status_with_orchestration_result(base, None, &result, now);
        let msg = out.conditions[0].message.as_deref().unwrap_or("");
        assert!(
            msg.contains("create batch/v1.jobs") && msg.contains("jobs.batch is forbidden"),
            "namespace-less RBAC message should still carry verb_resource + kube message: {msg}",
        );
    }

    #[test]
    fn t022_preserves_last_transition_time_when_reason_unchanged() {
        let earlier = Utc::now() - ChronoDuration::seconds(60);
        let later = Utc::now();
        let base_earlier = desired_status(&valid_spec(), earlier, None);
        let first = status_with_orchestration_result(
            base_earlier,
            None,
            &OrchestrationResult::Spawned { distinct_images: 2 },
            earlier,
        );

        let base_later = desired_status(&valid_spec(), later, Some(&first));
        let second = status_with_orchestration_result(
            base_later,
            Some(&first),
            &OrchestrationResult::Spawned { distinct_images: 2 },
            later,
        );

        assert_eq!(
            first.conditions[0].last_transition_time, second.conditions[0].last_transition_time,
            "lastTransitionTime stays put when (Ready=False, Scanning) is unchanged",
        );
    }

    #[test]
    fn t022_advances_last_transition_time_when_reason_changes() {
        let earlier = Utc::now() - ChronoDuration::seconds(60);
        let later = Utc::now();
        let base_earlier = desired_status(&valid_spec(), earlier, None);
        let first = status_with_orchestration_result(
            base_earlier,
            None,
            &OrchestrationResult::NoImagesInScope,
            earlier,
        );

        let base_later = desired_status(&valid_spec(), later, Some(&first));
        let second = status_with_orchestration_result(
            base_later,
            Some(&first),
            &OrchestrationResult::Spawned { distinct_images: 1 },
            later,
        );

        assert_eq!(
            second.conditions[0].last_transition_time,
            Some(later.to_rfc3339()),
            "lastTransitionTime must advance when reason changes (NoImagesInScope → Scanning)",
        );
    }
}
