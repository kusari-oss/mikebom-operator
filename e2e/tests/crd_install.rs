//! Constitution VI E2E: assert that `helm install charts/mikebom-operator/`
//! registers the NamespaceScan CRD in a kind cluster.
//!
//! Gated behind `MIKEBOM_OPERATOR_E2E=1`. Requires `helm` and `kubectl` on
//! PATH and a running kind cluster named `mikebom-operator-e2e` (create
//! with `kind create cluster --config e2e/kind-cluster.yaml`).

use std::process::{Command, Output};

const CLUSTER_NAME: &str = "mikebom-operator-e2e";
const RELEASE_NAME: &str = "mikebom-operator-crd-install";
const NAMESPACE: &str = "kusari-operator-crd-install";

fn run(args: &[&str]) -> Output {
    Command::new(args[0])
        .args(&args[1..])
        .output()
        .unwrap_or_else(|err| panic!("failed to spawn `{}`: {err}", args[0]))
}

fn kube_context() -> String {
    format!("kind-{CLUSTER_NAME}")
}

#[test]
fn helm_install_registers_crd() {
    if std::env::var("MIKEBOM_OPERATOR_E2E").ok().as_deref() != Some("1") {
        eprintln!("MIKEBOM_OPERATOR_E2E unset; skipping kind-based E2E.");
        return;
    }

    let context = kube_context();

    // Best-effort cleanup of any prior install. Ignore failures.
    let _ = run(&[
        "helm",
        "uninstall",
        RELEASE_NAME,
        "-n",
        NAMESPACE,
        "--kube-context",
        &context,
    ]);
    let _ = run(&[
        "kubectl",
        "delete",
        "namespace",
        NAMESPACE,
        "--context",
        &context,
        "--ignore-not-found",
    ]);

    let install = run(&[
        "helm",
        "install",
        RELEASE_NAME,
        "charts/mikebom-operator",
        "-n",
        NAMESPACE,
        "--create-namespace",
        "--kube-context",
        &context,
        "--wait",
        "--timeout",
        "60s",
    ]);
    assert!(
        install.status.success(),
        "helm install failed: {}\n--- stderr ---\n{}",
        install.status,
        String::from_utf8_lossy(&install.stderr),
    );

    let get_crd = run(&[
        "kubectl",
        "get",
        "crd",
        "namespacescans.kusari.dev",
        "--context",
        &context,
    ]);
    assert!(
        get_crd.status.success(),
        "kubectl get crd namespacescans.kusari.dev failed: {}\n--- stderr ---\n{}",
        get_crd.status,
        String::from_utf8_lossy(&get_crd.stderr),
    );

    // Cleanup.
    let _ = run(&[
        "helm",
        "uninstall",
        RELEASE_NAME,
        "-n",
        NAMESPACE,
        "--kube-context",
        &context,
    ]);
    let _ = run(&[
        "kubectl",
        "delete",
        "namespace",
        NAMESPACE,
        "--context",
        &context,
        "--ignore-not-found",
    ]);
}
