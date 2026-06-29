use std::env;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use futures::StreamExt;
use kube::{
    runtime::{watcher, Controller},
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
            let ctx = Arc::new(Ctx {
                client,
                operator_namespace,
            });

            info!(
                event = "controller_starting",
                "spawning NamespaceScan controller"
            );

            Controller::new(api, watcher::Config::default())
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
