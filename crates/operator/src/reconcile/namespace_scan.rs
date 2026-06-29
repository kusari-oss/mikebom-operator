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
use crate::reconcile::scan_orchestrator::{ensure_jobs, CrMetaSnapshot};
use crate::status::{desired_status, status_with_orchestration_result, REASON_INVALID_SPEC};

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
            status_with_orchestration_result(base_status, obj.status.as_ref(), &orchestration, now)
        }
    };

    let final_reason = new_status
        .conditions
        .first()
        .and_then(|c| c.reason.as_deref())
        .unwrap_or("unknown");

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

    Ok(Action::requeue(REQUEUE_INTERVAL))
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
