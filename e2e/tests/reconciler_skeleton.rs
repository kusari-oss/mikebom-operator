//! Constitution VI E2E for feature 002: asserts that the operator runs
//! in-cluster, acquires leader-election leadership, and acknowledges
//! `NamespaceScan` CRs via status conditions + `lastReconciledAt`.
//!
//! Covers feature 002's US1 (chart install + structured logs), US2 (valid CR
//! → NotYetReconciled; invalid CR → InvalidSpec; CR delete → no error logs),
//! and US3 observability (Lease visible with conventional holderIdentity).
//! Full failover (US3 with pod kill) is in `reconciler_failover.rs`, gated
//! separately so the slower test is opt-in.
//!
//! Gated behind `MIKEBOM_OPERATOR_E2E=1`. Prerequisites:
//!
//! 1. `kind create cluster --config e2e/kind-cluster.yaml`
//! 2. `docker build -t mikebom-operator:dev .`
//! 3. `kind load docker-image mikebom-operator:dev --name mikebom-operator-e2e`
//!
//! Then: `MIKEBOM_OPERATOR_E2E=1 cargo test --test reconciler_skeleton`

use std::process::{Command, Output};
use std::thread::sleep;
use std::time::{Duration, Instant};

const CLUSTER_NAME: &str = "mikebom-operator-e2e";
const RELEASE_NAME: &str = "mikebom-operator-reconciler-skeleton";
const NAMESPACE: &str = "kusari-operator-reconciler-skeleton";
const LOCAL_IMAGE_REPO: &str = "mikebom-operator";
const LOCAL_IMAGE_TAG: &str = "dev";

const VALID_CR_NAME: &str = "scan-prod";
const INVALID_CR_NAME: &str = "scan-invalid";

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

fn helm_install() {
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
            "60s",
            "--set",
            &format!("image.repository={LOCAL_IMAGE_REPO}"),
            "--set",
            &format!("image.tag={LOCAL_IMAGE_TAG}"),
            "--set",
            "image.pullPolicy=IfNotPresent",
        ],
        "helm install",
    );
}

fn operator_pod_name() -> String {
    let out = run_ok(
        &[
            "kubectl",
            "get",
            "pods",
            "-n",
            NAMESPACE,
            "-l",
            "app.kubernetes.io/name=mikebom-operator",
            "-o",
            "jsonpath={.items[0].metadata.name}",
            "--context",
            &kube_context(),
        ],
        "kubectl get pods (operator)",
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn operator_logs(pod: &str) -> String {
    let out = run_ok(
        &[
            "kubectl",
            "logs",
            pod,
            "-n",
            NAMESPACE,
            "--context",
            &kube_context(),
        ],
        "kubectl logs",
    );
    String::from_utf8(out.stdout).unwrap()
}

fn apply_yaml(yaml: &str) {
    let ctx = kube_context();
    let mut cmd = Command::new("kubectl");
    cmd.args(["apply", "-n", NAMESPACE, "-f", "-", "--context", &ctx])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().expect("failed to spawn kubectl apply");
    {
        use std::io::Write as _;
        let stdin = child.stdin.as_mut().expect("failed to open kubectl stdin");
        stdin
            .write_all(yaml.as_bytes())
            .expect("failed to write yaml to kubectl stdin");
    }
    let out = child.wait_with_output().expect("kubectl apply failed");
    assert!(
        out.status.success(),
        "kubectl apply failed:\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr),
    );
}

fn delete_cr(name: &str) {
    let _ = run(&[
        "kubectl",
        "delete",
        "namespacescan",
        name,
        "-n",
        NAMESPACE,
        "--context",
        &kube_context(),
        "--ignore-not-found",
        "--wait",
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

fn logs_contain_event(logs: &str, event: &str) -> bool {
    let needle = format!("\"event\":\"{event}\"");
    logs.lines().any(|line| line.contains(&needle))
}

fn logs_have_error(logs: &str) -> bool {
    logs.lines()
        .any(|line| line.contains("\"level\":\"ERROR\""))
}

fn valid_cr_yaml() -> String {
    format!(
        r#"apiVersion: kusari.dev/v1alpha1
kind: NamespaceScan
metadata:
  name: {VALID_CR_NAME}
spec:
  target:
    namespaces: [default]
    kinds: [Pod]
  schedule:
    cron: "0 */6 * * *"
  mikebomImage: ghcr.io/kusari-oss/mikebom:v0.1.0-alpha.64
  scanFormat: cyclonedx-json
  output:
    type: pvc
    pvc:
      claimName: scratch
"#
    )
}

fn invalid_cr_yaml() -> String {
    format!(
        r#"apiVersion: kusari.dev/v1alpha1
kind: NamespaceScan
metadata:
  name: {INVALID_CR_NAME}
spec:
  target:
    namespaces: []
    kinds: [Pod]
  schedule:
    cron: "0 */6 * * *"
  mikebomImage: ghcr.io/kusari-oss/mikebom:v0.1.0-alpha.64
  scanFormat: cyclonedx-json
  output:
    type: pvc
    pvc:
      claimName: scratch
"#
    )
}

#[test]
fn reconciler_skeleton_full_flow() {
    if std::env::var("MIKEBOM_OPERATOR_E2E").ok().as_deref() != Some("1") {
        eprintln!("MIKEBOM_OPERATOR_E2E unset; skipping kind-based E2E.");
        return;
    }

    cleanup();
    // FR-001 / SC-001: --wait --timeout 60s asserts Pod Ready < 60s.
    helm_install();

    let pod = operator_pod_name();
    assert!(!pod.is_empty(), "no operator pod found after helm install");

    // FR-009 / SC-005 (US1): structured JSON records with the expected event values.
    assert!(
        poll(Duration::from_secs(30), || logs_contain_event(
            &operator_logs(&pod),
            "startup"
        )),
        "operator logs missing structured event=startup record within 30s",
    );
    // FR-002 / US1: operator acquires leadership.
    assert!(
        poll(Duration::from_secs(30), || logs_contain_event(
            &operator_logs(&pod),
            "leader_acquired"
        )),
        "operator logs missing structured event=leader_acquired record within 30s",
    );

    // FR-007 / US3 observability: Lease visible with conventional holderIdentity.
    let holder = jsonpath("lease", "mikebom-operator-leader", "{.spec.holderIdentity}");
    assert!(
        holder.starts_with("mikebom-operator-"),
        "Lease holderIdentity {holder:?} doesn't match mikebom-operator-{{POD_NAME}} format",
    );

    // FR-003 / FR-004 / SC-002 / US2: valid CR → NotYetReconciled + lastReconciledAt < 10s.
    apply_yaml(&valid_cr_yaml());
    assert!(
        poll(Duration::from_secs(10), || {
            jsonpath(
                "namespacescan",
                VALID_CR_NAME,
                "{.status.conditions[?(@.type==\"Ready\")].reason}",
            ) == "NotYetReconciled"
        }),
        "NamespaceScan {VALID_CR_NAME}'s Ready condition didn't reach NotYetReconciled within 10s",
    );
    let last_reconciled = jsonpath("namespacescan", VALID_CR_NAME, "{.status.lastReconciledAt}");
    assert!(
        !last_reconciled.is_empty(),
        "NamespaceScan {VALID_CR_NAME}.status.lastReconciledAt is empty",
    );

    // FR-011 / US2: invalid CR → InvalidSpec < 10s.
    apply_yaml(&invalid_cr_yaml());
    assert!(
        poll(Duration::from_secs(10), || {
            jsonpath(
                "namespacescan",
                INVALID_CR_NAME,
                "{.status.conditions[?(@.type==\"Ready\")].reason}",
            ) == "InvalidSpec"
        }),
        "NamespaceScan {INVALID_CR_NAME}'s Ready condition didn't reach InvalidSpec within 10s",
    );

    // FR-012 / US2: CR deletion produces no ERROR-level operator logs.
    delete_cr(VALID_CR_NAME);
    delete_cr(INVALID_CR_NAME);
    sleep(Duration::from_secs(5)); // give the operator a moment to process
    let post_delete_logs = operator_logs(&pod);
    assert!(
        !logs_have_error(&post_delete_logs),
        "operator emitted ERROR-level logs after CR deletion:\n{post_delete_logs}",
    );

    cleanup();
}
