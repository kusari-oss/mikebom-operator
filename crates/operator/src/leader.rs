//! Leader election via `coordination.k8s.io/v1.Lease`.
//!
//! Hand-rolled per research §R1 — kube-rs 0.97 doesn't ship a leader-election
//! helper we want to depend on at this layer. The contract:
//!
//! * Exactly one replica holds the lease at any moment.
//! * The holder renews `renewTime` at roughly 1/3 the lease duration.
//! * If renewal fails, the operator exits non-zero so Kubernetes restarts it;
//!   the lease times out within `leaseDurationSeconds` and another replica
//!   acquires.
//!
//! See `specs/002-reconciler-skeleton/contracts/leader-election.md` for the
//! user-visible contract this module implements.

use std::time::Duration;

use anyhow::{anyhow, Context as _, Result};
use chrono::Utc;
use k8s_openapi::api::coordination::v1::{Lease, LeaseSpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{MicroTime, ObjectMeta};
use kube::{
    api::{Patch, PatchParams, PostParams},
    Api, Client,
};
use serde_json::json;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::{error, info, warn};

const LEASE_DURATION_SECONDS: i32 = 15;
const RENEW_INTERVAL: Duration = Duration::from_secs(5);
const ACQUIRE_RETRY_INTERVAL: Duration = Duration::from_secs(3);
const FIELD_MANAGER: &str = "mikebom-operator";

/// Configuration for the leader-election lease.
#[derive(Debug, Clone)]
pub struct LeaderConfig {
    /// Namespace the Lease lives in (typically the operator's own namespace).
    pub namespace: String,
    /// Lease object name; must be DNS-1123 compatible.
    pub lease_name: String,
    /// Unique identity of this replica — used in `holderIdentity`. Conventionally
    /// `mikebom-operator-{POD_NAME}`.
    pub identity: String,
}

/// Acquire the lease (blocking until held), spawn a background renewer, then
/// run `body`. Returns when `body` completes. If the background renewer fails,
/// the process exits non-zero (Kubernetes will restart the pod).
pub async fn run_with_leadership<F, Fut>(
    client: Client,
    config: LeaderConfig,
    body: F,
) -> Result<()>
where
    F: FnOnce(Client) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let leases: Api<Lease> = Api::namespaced(client.clone(), &config.namespace);

    // Block until we acquire the lease.
    acquire(&leases, &config).await?;
    info!(
        event = "leader_acquired",
        lease = %config.lease_name,
        namespace = %config.namespace,
        identity = %config.identity,
        "acquired leader-election lease",
    );

    // Spawn renewer.
    let renewer = spawn_renewer(leases.clone(), config.clone());

    // Run the body while we hold leadership.
    let body_result = body(client).await;

    // Stop the renewer.
    renewer.abort();

    body_result
}

/// Block until we hold the lease.
async fn acquire(leases: &Api<Lease>, config: &LeaderConfig) -> Result<()> {
    loop {
        match try_acquire(leases, config).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                warn!(
                    event = "leader_acquire_waiting",
                    error = %format!("{e:#}"),
                    "waiting to acquire lease",
                );
                sleep(ACQUIRE_RETRY_INTERVAL).await;
            }
        }
    }
}

/// One acquisition attempt. Returns `Ok(())` if we hold the lease after the
/// attempt; `Err` if the lease is held by someone else and not yet expired.
async fn try_acquire(leases: &Api<Lease>, config: &LeaderConfig) -> Result<()> {
    let now = Utc::now();
    let micro = MicroTime(now);

    match leases.get_opt(&config.lease_name).await? {
        None => {
            // Lease doesn't exist — create it with us as holder.
            let lease = Lease {
                metadata: ObjectMeta {
                    name: Some(config.lease_name.clone()),
                    namespace: Some(config.namespace.clone()),
                    ..Default::default()
                },
                spec: Some(LeaseSpec {
                    acquire_time: Some(micro.clone()),
                    renew_time: Some(micro),
                    holder_identity: Some(config.identity.clone()),
                    lease_duration_seconds: Some(LEASE_DURATION_SECONDS),
                    ..Default::default()
                }),
            };
            leases
                .create(&PostParams::default(), &lease)
                .await
                .context("failed to create leader-election Lease")?;
            Ok(())
        }
        Some(existing) => {
            let spec = existing.spec.as_ref();
            let current_holder = spec.and_then(|s| s.holder_identity.as_deref());
            let renew_time = spec.and_then(|s| s.renew_time.as_ref());
            let duration_secs = spec
                .and_then(|s| s.lease_duration_seconds)
                .unwrap_or(LEASE_DURATION_SECONDS);

            let we_hold = current_holder == Some(config.identity.as_str());
            let expired = match renew_time {
                Some(t) => (Utc::now() - t.0).num_seconds() > duration_secs as i64,
                None => true,
            };

            if !we_hold && !expired {
                return Err(anyhow!(
                    "lease held by {:?}; not yet expired (duration={}s)",
                    current_holder,
                    duration_secs,
                ));
            }

            // Patch with us as holder + fresh acquireTime/renewTime.
            let patch = json!({
                "spec": {
                    "acquireTime": micro,
                    "renewTime": micro,
                    "holderIdentity": config.identity,
                    "leaseDurationSeconds": LEASE_DURATION_SECONDS,
                }
            });
            leases
                .patch(
                    &config.lease_name,
                    &PatchParams::default(),
                    &Patch::Merge(&patch),
                )
                .await
                .context("failed to patch lease for acquisition")?;
            Ok(())
        }
    }
}

/// Background renewer: every `RENEW_INTERVAL`, PATCH the Lease's `renewTime`.
/// On any error, exits the process so Kubernetes restarts the pod.
fn spawn_renewer(leases: Api<Lease>, config: LeaderConfig) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            sleep(RENEW_INTERVAL).await;
            let micro = MicroTime(Utc::now());
            let patch = json!({
                "spec": {
                    "renewTime": micro,
                    "holderIdentity": config.identity,
                }
            });
            if let Err(e) = leases
                .patch(
                    &config.lease_name,
                    &PatchParams::apply(FIELD_MANAGER).force(),
                    &Patch::Merge(&patch),
                )
                .await
            {
                error!(
                    event = "lease_renewal_failed",
                    error = %format!("{e:#}"),
                    "failed to renew lease; exiting process so kubernetes restarts the pod",
                );
                std::process::exit(1);
            }
        }
    })
}
