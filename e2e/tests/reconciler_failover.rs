//! Feature 002 US3: leader-election failover within 30 seconds (SC-003).
//!
//! Scales the operator to 2 replicas, kills the current leader, asserts a new
//! leader picks up the Lease and an existing CR's `lastReconciledAt` refreshes
//! within the SC-003 budget.
//!
//! Gated behind `MIKEBOM_OPERATOR_E2E_FAILOVER=1` (NOT the same as the
//! standard `MIKEBOM_OPERATOR_E2E=1` — failover is slower and flakier than
//! the steady-state E2E, so it's opt-in). Prerequisites match
//! `reconciler_skeleton.rs`: kind cluster + dev image loaded.

use std::process::{Command, Output};
use std::thread::sleep;
use std::time::{Duration, Instant};

const CLUSTER_NAME: &str = "mikebom-operator-e2e";
const RELEASE_NAME: &str = "mikebom-operator-reconciler-failover";
const NAMESPACE: &str = "kusari-operator-reconciler-failover";
const LOCAL_IMAGE_REPO: &str = "mikebom-operator";
const LOCAL_IMAGE_TAG: &str = "dev";
const CR_NAME: &str = "scan-prod";

fn run(args: &[&str]) -> Output {
    Command::new(args[0])
        .args(&args[1..])
        .output()
        .unwrap_or_else(|err| panic!("failed to spawn `{}`: {err}", args[0]))
}

fn run_ok(args: &[&str], context: &str) -> Output {
    let out = run(args);
    assert!(
        out.status.success(),
        "{context}: command failed: {args:?}\nstderr:\n{}\nstdout:\n{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout),
    );
    out
}

fn kube_context() -> String {
    format!("kind-{CLUSTER_NAME}")
}

fn cleanup() {
    let ctx = kube_context();
    let _ = run(&[
        "helm",
        "uninstall",
        RELEASE_NAME,
        "-n",
        NAMESPACE,
        "--kube-context",
        &ctx,
    ]);
    let _ = run(&[
        "kubectl",
        "delete",
        "namespace",
        NAMESPACE,
        "--context",
        &ctx,
        "--ignore-not-found",
        "--wait=false",
    ]);
}

fn helm_install_two_replicas() {
    let ctx = kube_context();
    run_ok(
        &[
            "helm",
            "install",
            RELEASE_NAME,
            "charts/mikebom-operator",
            "-n",
            NAMESPACE,
            "--create-namespace",
            "--kube-context",
            &ctx,
            "--wait",
            "--timeout",
            "120s",
            "--set",
            &format!("image.repository={LOCAL_IMAGE_REPO}"),
            "--set",
            &format!("image.tag={LOCAL_IMAGE_TAG}"),
            "--set",
            "image.pullPolicy=IfNotPresent",
            "--set",
            "replicaCount=2",
        ],
        "helm install (2 replicas)",
    );
    // The chart's deployment.yaml hardcodes replicas: 1; force scale.
    let _ = run(&[
        "kubectl",
        "scale",
        "deployment",
        "--replicas=2",
        "-l",
        "app.kubernetes.io/name=mikebom-operator",
        "-n",
        NAMESPACE,
        "--context",
        &ctx,
    ]);
}

fn jsonpath(resource: &str, name: &str, path: &str) -> String {
    let out = run(&[
        "kubectl",
        "get",
        resource,
        name,
        "-n",
        NAMESPACE,
        "-o",
        &format!("jsonpath={path}"),
        "--context",
        &kube_context(),
    ]);
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn apply_valid_cr() {
    let yaml = format!(
        r#"apiVersion: kusari.dev/v1alpha1
kind: NamespaceScan
metadata:
  name: {CR_NAME}
spec:
  target:
    namespaces: [default]
    kinds: [Pod]
  schedule:
    cron: "0 */6 * * *"
  mikebomImage: ghcr.io/kusari-oss/mikebom:v0.1.0-alpha.66
  scanFormat: cyclonedx-json
  output:
    type: pvc
    pvc:
      claimName: scratch
"#
    );

    let mut cmd = Command::new("kubectl");
    cmd.args([
        "apply",
        "-n",
        NAMESPACE,
        "-f",
        "-",
        "--context",
        &kube_context(),
    ])
    .stdin(std::process::Stdio::piped())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().expect("failed to spawn kubectl apply");
    {
        use std::io::Write as _;
        let stdin = child.stdin.as_mut().expect("failed to open kubectl stdin");
        stdin
            .write_all(yaml.as_bytes())
            .expect("failed to write yaml");
    }
    let out = child.wait_with_output().expect("kubectl apply failed");
    assert!(
        out.status.success(),
        "kubectl apply failed:\n{}",
        String::from_utf8_lossy(&out.stderr),
    );
}

fn poll<F>(timeout: Duration, mut check: F) -> bool
where
    F: FnMut() -> bool,
{
    let start = Instant::now();
    while start.elapsed() < timeout {
        if check() {
            return true;
        }
        sleep(Duration::from_secs(1));
    }
    false
}

fn current_holder() -> String {
    jsonpath("lease", "mikebom-operator-leader", "{.spec.holderIdentity}")
}

#[test]
fn failover_within_30s() {
    if std::env::var("MIKEBOM_OPERATOR_E2E_FAILOVER")
        .ok()
        .as_deref()
        != Some("1")
    {
        eprintln!("MIKEBOM_OPERATOR_E2E_FAILOVER unset; skipping leader-election failover E2E.",);
        return;
    }

    cleanup();
    helm_install_two_replicas();
    apply_valid_cr();

    // Wait for the Lease to settle on some leader, and for the CR to get an initial reconcile.
    assert!(
        poll(Duration::from_secs(60), || {
            !current_holder().is_empty()
                && !jsonpath("namespacescan", CR_NAME, "{.status.lastReconciledAt}").is_empty()
        }),
        "Lease never acquired a holder OR CR was never reconciled within 60s",
    );

    let initial_holder = current_holder();
    let initial_reconciled = jsonpath("namespacescan", CR_NAME, "{.status.lastReconciledAt}");

    // initial_holder is of the form `mikebom-operator-{POD_NAME}`. Strip the prefix.
    let leader_pod = initial_holder
        .strip_prefix("mikebom-operator-")
        .expect("holderIdentity missing expected prefix")
        .to_string();

    // SC-003: kill the leader and assert a new holder takes over within 30s.
    run_ok(
        &[
            "kubectl",
            "delete",
            "pod",
            &leader_pod,
            "-n",
            NAMESPACE,
            "--context",
            &kube_context(),
            "--grace-period=0",
            "--force",
        ],
        "kubectl delete leader pod",
    );

    assert!(
        poll(Duration::from_secs(30), || {
            let h = current_holder();
            !h.is_empty() && h != initial_holder
        }),
        "Lease holderIdentity did not change within 30s after killing the leader pod",
    );

    // FR-008 / SC-003: existing CR's lastReconciledAt advances within a further 30s.
    assert!(
        poll(Duration::from_secs(30), || {
            jsonpath("namespacescan", CR_NAME, "{.status.lastReconciledAt}") != initial_reconciled
        }),
        "NamespaceScan lastReconciledAt did not refresh within 30s of failover",
    );

    cleanup();
}
