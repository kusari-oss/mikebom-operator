use std::env;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use futures::StreamExt;
use k8s_openapi::api::batch::v1::Job;
use kube::{
    runtime::{reflector::ObjectRef, watcher, Controller},
    Api, Client,
};
use operator::crds::namespace_scan::NamespaceScan;
use operator::leader::{run_with_leadership, LeaderConfig};
use operator::reconcile::namespace_scan::{error_policy, reconcile, Ctx};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    // Structured JSON logs per FR-009 / SC-005.
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let pod_name = env::var("POD_NAME").unwrap_or_else(|_| "unknown".to_string());
    let pod_namespace = env::var("POD_NAMESPACE").unwrap_or_else(|_| "default".to_string());
    let lease_name =
        env::var("MIKEBOM_LEADER_LEASE").unwrap_or_else(|_| "mikebom-operator-leader".to_string());

    info!(
        event = "startup",
        pod_name = %pod_name,
        pod_namespace = %pod_namespace,
        lease_name = %lease_name,
        "mikebom-operator starting",
    );

    let client = Client::try_default()
        .await
        .context("failed to create kube client (is the operator running outside a cluster?)")?;

    let leader_config = LeaderConfig {
        namespace: pod_namespace.clone(),
        lease_name,
        identity: format!("mikebom-operator-{pod_name}"),
    };

    let operator_namespace = pod_namespace;
    run_with_leadership(client.clone(), leader_config, move |client| {
        let operator_namespace = operator_namespace.clone();
        async move {
            let api: Api<NamespaceScan> = Api::all(client.clone());
            // Feature 008: watch Jobs in the operator's namespace. The
            // mapping fn extracts the owning CR's name from the
            // `kusari.dev/namespace-scan` label (FR-001 + FR-011 — unowned
            // events return None and are ignored).
            let jobs_api: Api<Job> = Api::namespaced(client.clone(), &operator_namespace);
            let ctx = Arc::new(Ctx {
                client,
                operator_namespace: operator_namespace.clone(),
            });

            info!(
                event = "controller_starting",
                "spawning NamespaceScan controller with Job watch (feature 008)"
            );

            let mapper_namespace = operator_namespace.clone();
            Controller::new(api, watcher::Config::default())
                .watches(
                    jobs_api,
                    watcher::Config::default(),
                    move |job: Job| -> Option<ObjectRef<NamespaceScan>> {
                        job_to_cr_request(&job, &mapper_namespace)
                    },
                )
                .run(reconcile, error_policy, ctx)
                .for_each(|res| async move {
                    if let Err(e) = res {
                        warn!(
                            event = "controller_runtime_error",
                            error = %format!("{e:#}"),
                            "controller stream emitted an error",
                        );
                    }
                })
                .await;

            Ok(())
        }
    })
    .await?;

    Ok(())
}

/// Translate a Job watch event to the owning NamespaceScan reconcile request.
/// Returns `None` for Jobs without the `kusari.dev/namespace-scan` label
/// (FR-011 — ignore unowned events).
fn job_to_cr_request(job: &Job, operator_namespace: &str) -> Option<ObjectRef<NamespaceScan>> {
    let labels = job.metadata.labels.as_ref()?;
    let cr_name = labels.get("kusari.dev/namespace-scan")?;
    Some(ObjectRef::new(cr_name).within(operator_namespace))
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use std::collections::BTreeMap;

    fn labeled_job(cr_name: &str) -> Job {
        let mut labels = BTreeMap::new();
        labels.insert("kusari.dev/namespace-scan".to_string(), cr_name.to_string());
        Job {
            metadata: ObjectMeta {
                name: Some(format!("nsscan-{cr_name}-abc1234")),
                labels: Some(labels),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn unlabeled_job() -> Job {
        Job {
            metadata: ObjectMeta {
                name: Some("random-job".to_string()),
                labels: None,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn t011_job_to_cr_request_labeled_returns_some_with_namespace_and_name() {
        let job = labeled_job("scan-prod");
        let req = job_to_cr_request(&job, "kusari-operator")
            .expect("labeled Job should produce ObjectRef");
        assert_eq!(req.name, "scan-prod");
        assert_eq!(req.namespace.as_deref(), Some("kusari-operator"));
    }

    #[test]
    fn t011_job_to_cr_request_unlabeled_returns_none() {
        let job = unlabeled_job();
        let req = job_to_cr_request(&job, "kusari-operator");
        assert!(req.is_none(), "unlabeled Job MUST be ignored (FR-011)");
    }

    #[test]
    fn t011_job_to_cr_request_label_with_different_key_returns_none() {
        // A Job labeled with something other than `kusari.dev/namespace-scan`
        // (e.g., k8s system labels) MUST return None.
        let mut labels = BTreeMap::new();
        labels.insert("app".to_string(), "unrelated".to_string());
        let job = Job {
            metadata: ObjectMeta {
                labels: Some(labels),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(job_to_cr_request(&job, "kusari-operator").is_none());
    }
}
