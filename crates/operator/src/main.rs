use anyhow::Result;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tracing::info!("mikebom-operator starting");

    // Reconciler wiring lands in feature 002 (per plan §10):
    //   - kube::Client::try_default()
    //   - kube::runtime::reflector::Lease for leader election
    //   - Controller::new(api, watcher::Config::default()).run(...)

    Ok(())
}
