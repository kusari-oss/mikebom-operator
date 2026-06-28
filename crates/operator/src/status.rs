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
}
