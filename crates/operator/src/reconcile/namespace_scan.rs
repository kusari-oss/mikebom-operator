//! Reconciler for `NamespaceScan` CRs.
//!
//! Per spec FRs 003/004/010/011/012: every CR gets a `Ready=False` condition
//! and a refreshed `lastReconciledAt` on every reconcile cycle. Idempotency
//! and InvalidSpec rules are computed by [`crate::status::desired_status`];
//! this module just patches the result onto the CR and decides when to
//! requeue.

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
use crate::status::desired_status;

const REQUEUE_INTERVAL: Duration = Duration::from_secs(300);
const ERROR_REQUEUE: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
}

#[derive(Debug, Error)]
pub enum ReconcileError {
    #[error("kube API error: {0}")]
    Kube(#[from] kube::Error),
}

pub async fn reconcile(obj: Arc<NamespaceScan>, ctx: Arc<Ctx>) -> Result<Action, ReconcileError> {
    let name = obj.name_any();
    let namespace = obj.namespace().unwrap_or_default();

    let api: Api<NamespaceScan> = if namespace.is_empty() {
        Api::all(ctx.client.clone())
    } else {
        Api::namespaced(ctx.client.clone(), &namespace)
    };

    let new_status = desired_status(&obj.spec, Utc::now(), obj.status.as_ref());

    let reason = new_status
        .conditions
        .first()
        .and_then(|c| c.reason.as_deref())
        .unwrap_or("unknown");

    info!(
        event = "reconcile",
        namespace_scan = %name,
        namespace = %namespace,
        reason = %reason,
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
