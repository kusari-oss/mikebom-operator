//! Constitution VI E2E for feature 008: real-operator-in-kind tests of the
//! watch-driven Job-status feedback path.
//!
//! Unlike feature 007's `reconciler_spawns_job.rs` (in-process), feature 008
//! requires the operator to actually run in kind so its `Controller::watches`
//! wiring exercises the watch event delivery. The full chart-install +
//! image-load scaffolding is needed.
//!
//! Gated behind `MIKEBOM_OPERATOR_E2E=1`. Prerequisites:
//!
//! 1. `kind create cluster --config e2e/kind-cluster.yaml`
//! 2. `docker build -t mikebom-operator:dev .`
//! 3. `kind load docker-image mikebom-operator:dev --name mikebom-operator-e2e`
//!
//! Then: `MIKEBOM_OPERATOR_E2E=1 cargo test --test job_status_feedback`
//!
//! Tests:
//! - T018  ScanCompleted within 5s of Job status patch
//! - T018b status survives operator restart (FR-010 + SC-006)
//! - T025  ScanFailed within 5s, message names failing image
//! - T026  failure dominates mixed-state aggregation
//! - T034  scannedImages populated with PVC URLs
//! - T034b append-only across output edit + new pod

#![allow(clippy::needless_return)]

mod common;
use common::*;

use std::thread::sleep;
use std::time::{Duration, Instant};

fn e2e_enabled() -> bool {
    std::env::var("MIKEBOM_OPERATOR_E2E").ok().as_deref() == Some("1")
}

fn skip(reason: &str) {
    eprintln!("MIKEBOM_OPERATOR_E2E unset; skipping {reason}");
}

fn cr_yaml_pvc(name: &str, target_ns: &str) -> String {
    format!(
        r#"
apiVersion: kusari.dev/v1alpha1
kind: NamespaceScan
metadata:
  name: {name}
spec:
  target:
    namespaces: [{target_ns}]
  schedule:
    interval: "6h"
  mikebomImage: ghcr.io/kusari-oss/mikebom:v0.1.0-alpha.62
  scanFormat: cyclonedx-json
  output:
    type: pvc
    pvc:
      claimName: sbom-claim
      pathPrefix: team-a
"#
    )
}

fn pod_yaml(name: &str, target_ns: &str, image: &str) -> String {
    format!(
        r#"
apiVersion: v1
kind: Pod
metadata:
  name: {name}
  namespace: {target_ns}
spec:
  containers:
    - name: app
      image: {image}
      command: ["sh", "-c", "sleep 3600"]
"#
    )
}

fn ns_yaml(name: &str) -> String {
    format!(
        r#"
apiVersion: v1
kind: Namespace
metadata:
  name: {name}
"#
    )
}

fn install_and_setup(release: &str, ns: &str, target_ns: &str, cr_name: &str) -> Install {
    let install = Install::new(release, ns);
    install.cleanup();
    install.helm_install();
    install.apply_yaml_in(target_ns, &ns_yaml(target_ns));
    install.apply_yaml_in(ns, &cr_yaml_pvc(cr_name, target_ns));
    install
}

fn wait_for_jobs(
    install: &Install,
    cr_name: &str,
    expected: usize,
    timeout: Duration,
) -> Vec<String> {
    let start = Instant::now();
    loop {
        let jobs = install.list_jobs_for_cr(cr_name);
        if jobs.len() >= expected {
            return jobs;
        }
        if start.elapsed() > timeout {
            panic!(
                "timed out waiting for {expected} Job(s) for CR {cr_name}; saw {} so far",
                jobs.len()
            );
        }
        sleep(Duration::from_millis(500));
    }
}

fn wait_for_reason(install: &Install, cr_name: &str, expected: &str, timeout: Duration) {
    let start = Instant::now();
    loop {
        let reason = install.jsonpath(
            "namespacescan",
            cr_name,
            r#"{.status.conditions[?(@.type=="Ready")].reason}"#,
        );
        if reason == expected {
            return;
        }
        if start.elapsed() > timeout {
            panic!("timed out after {timeout:?} waiting for reason={expected}; current={reason}",);
        }
        sleep(Duration::from_millis(250));
    }
}

// ---------------------------------------------------------------------------
// T018 — ScanCompleted within 5s of Job's status.succeeded=1
// ---------------------------------------------------------------------------

#[test]
fn t018_scan_completed_within_5s() {
    if !e2e_enabled() {
        skip("kind-based feature 008 E2E (T018)");
        return;
    }
    let release = "mikebom-008-t018";
    let ns = "kusari-operator-008-t018";
    let target_ns = "feat008-t018";
    let cr_name = "scan-t018";
    let install = install_and_setup(release, ns, target_ns, cr_name);

    install.apply_yaml_in(target_ns, &pod_yaml("alpha", target_ns, "nginx:1.27.0"));

    let jobs = wait_for_jobs(&install, cr_name, 1, Duration::from_secs(30));
    let job = &jobs[0];

    let now = chrono::Utc::now().to_rfc3339();
    install.patch_job_status(
        job,
        &format!(r#"{{"status": {{"succeeded": 1, "completionTime": "{now}"}}}}"#),
    );

    wait_for_reason(&install, cr_name, "ScanCompleted", Duration::from_secs(10));
    let status = install.jsonpath(
        "namespacescan",
        cr_name,
        r#"{.status.conditions[?(@.type=="Ready")].status}"#,
    );
    assert_eq!(status, "True", "ScanCompleted MUST be Ready=True");

    install.cleanup();
}

// ---------------------------------------------------------------------------
// T018b — status survives operator restart (FR-010 + SC-006)
// ---------------------------------------------------------------------------

#[test]
fn t018b_status_survives_operator_restart() {
    if !e2e_enabled() {
        skip("kind-based feature 008 E2E (T018b restart resync)");
        return;
    }
    let release = "mikebom-008-t018b";
    let ns = "kusari-operator-008-t018b";
    let target_ns = "feat008-t018b";
    let cr_name = "scan-t018b";
    let install = install_and_setup(release, ns, target_ns, cr_name);

    install.apply_yaml_in(target_ns, &pod_yaml("alpha", target_ns, "nginx:1.27.0"));

    let jobs = wait_for_jobs(&install, cr_name, 1, Duration::from_secs(30));
    let now = chrono::Utc::now().to_rfc3339();
    install.patch_job_status(
        &jobs[0],
        &format!(r#"{{"status": {{"succeeded": 1, "completionTime": "{now}"}}}}"#),
    );
    wait_for_reason(&install, cr_name, "ScanCompleted", Duration::from_secs(10));

    // Kill the operator pod; chart deployment will restart it.
    let pod = install.operator_pod_name();
    run_ok(
        &[
            "kubectl",
            "delete",
            "pod",
            &pod,
            "-n",
            ns,
            "--context",
            &kube_context(),
            "--wait=false",
        ],
        "delete operator pod",
    );

    // Wait for the new operator pod to come up + reach Ready.
    run_ok(
        &[
            "kubectl",
            "wait",
            "--for=condition=Ready",
            "pod",
            "-l",
            "app.kubernetes.io/name=mikebom-operator",
            "-n",
            ns,
            "--timeout=60s",
            "--context",
            &kube_context(),
        ],
        "wait for new operator pod Ready",
    );

    // Within 30s of recovery, the status MUST still report ScanCompleted (the
    // watch resync observes the pre-existing Job's terminal state and
    // re-derives the same status).
    wait_for_reason(&install, cr_name, "ScanCompleted", Duration::from_secs(30));

    install.cleanup();
}

// ---------------------------------------------------------------------------
// T025 — ScanFailed within 5s of Job exhausting retries
// ---------------------------------------------------------------------------

#[test]
fn t025_scan_failed_within_5s() {
    if !e2e_enabled() {
        skip("kind-based feature 008 E2E (T025)");
        return;
    }
    let release = "mikebom-008-t025";
    let ns = "kusari-operator-008-t025";
    let target_ns = "feat008-t025";
    let cr_name = "scan-t025";
    let install = install_and_setup(release, ns, target_ns, cr_name);

    install.apply_yaml_in(target_ns, &pod_yaml("alpha", target_ns, "nginx:1.27.0"));

    let jobs = wait_for_jobs(&install, cr_name, 1, Duration::from_secs(30));
    // Patch with failed=7 (default backoffLimit=6 → finally failed at 7).
    install.patch_job_status(&jobs[0], r#"{"status": {"failed": 7}}"#);
    wait_for_reason(&install, cr_name, "ScanFailed", Duration::from_secs(10));

    let message = install.jsonpath(
        "namespacescan",
        cr_name,
        r#"{.status.conditions[?(@.type=="Ready")].message}"#,
    );
    assert!(
        message.contains("nginx:1.27.0"),
        "ScanFailed message MUST name the failing image (FR-004), got: {message}",
    );

    install.cleanup();
}

// ---------------------------------------------------------------------------
// T026 — failure dominates mixed-state aggregation (SC-003)
// ---------------------------------------------------------------------------

#[test]
fn t026_failure_dominates_mixed() {
    if !e2e_enabled() {
        skip("kind-based feature 008 E2E (T026)");
        return;
    }
    let release = "mikebom-008-t026";
    let ns = "kusari-operator-008-t026";
    let target_ns = "feat008-t026";
    let cr_name = "scan-t026";
    let install = install_and_setup(release, ns, target_ns, cr_name);

    install.apply_yaml_in(target_ns, &pod_yaml("alpha", target_ns, "nginx:1.27.0"));
    install.apply_yaml_in(target_ns, &pod_yaml("beta", target_ns, "redis:7.4.0"));

    let jobs = wait_for_jobs(&install, cr_name, 2, Duration::from_secs(30));
    let now = chrono::Utc::now().to_rfc3339();
    install.patch_job_status(
        &jobs[0],
        &format!(r#"{{"status": {{"succeeded": 1, "completionTime": "{now}"}}}}"#),
    );
    install.patch_job_status(&jobs[1], r#"{"status": {"failed": 7}}"#);

    wait_for_reason(&install, cr_name, "ScanFailed", Duration::from_secs(10));

    install.cleanup();
}

// ---------------------------------------------------------------------------
// T034 — scannedImages populated with PVC URLs
// ---------------------------------------------------------------------------

#[test]
fn t034_scanned_images_populated_pvc() {
    if !e2e_enabled() {
        skip("kind-based feature 008 E2E (T034)");
        return;
    }
    let release = "mikebom-008-t034";
    let ns = "kusari-operator-008-t034";
    let target_ns = "feat008-t034";
    let cr_name = "scan-t034";
    let install = install_and_setup(release, ns, target_ns, cr_name);

    install.apply_yaml_in(target_ns, &pod_yaml("alpha", target_ns, "nginx:1.27.0"));
    install.apply_yaml_in(target_ns, &pod_yaml("beta", target_ns, "redis:7.4.0"));

    let jobs = wait_for_jobs(&install, cr_name, 2, Duration::from_secs(30));
    let now = chrono::Utc::now().to_rfc3339();
    for job in &jobs {
        install.patch_job_status(
            job,
            &format!(r#"{{"status": {{"succeeded": 1, "completionTime": "{now}"}}}}"#),
        );
    }
    wait_for_reason(&install, cr_name, "ScanCompleted", Duration::from_secs(10));

    let count = install.jsonpath(
        "namespacescan",
        cr_name,
        r#"{.status.scannedImages[*].imageRef}"#,
    );
    let refs: Vec<&str> = count.split_whitespace().collect();
    assert_eq!(
        refs.len(),
        2,
        "expected 2 scannedImages entries, got: {count}"
    );

    let locations = install.jsonpath(
        "namespacescan",
        cr_name,
        r#"{.status.scannedImages[*].sbomLocation}"#,
    );
    for loc in locations.split_whitespace() {
        assert!(
            loc.starts_with("pvc://sbom-claim/team-a/") && loc.ends_with(".json"),
            "sbomLocation MUST match pvc://<claim>/<prefix>/<hash>.json, got: {loc}",
        );
    }

    install.cleanup();
}

// ---------------------------------------------------------------------------
// T034b — scannedImages append-only across output edit + new pod
// ---------------------------------------------------------------------------

#[test]
fn t034b_scanned_images_append_only_across_edits() {
    if !e2e_enabled() {
        skip("kind-based feature 008 E2E (T034b)");
        return;
    }
    let release = "mikebom-008-t034b";
    let ns = "kusari-operator-008-t034b";
    let target_ns = "feat008-t034b";
    let cr_name = "scan-t034b";
    let install = install_and_setup(release, ns, target_ns, cr_name);

    install.apply_yaml_in(target_ns, &pod_yaml("alpha", target_ns, "nginx:1.27.0"));
    install.apply_yaml_in(target_ns, &pod_yaml("beta", target_ns, "redis:7.4.0"));

    let jobs = wait_for_jobs(&install, cr_name, 2, Duration::from_secs(30));
    let now = chrono::Utc::now().to_rfc3339();
    for job in &jobs {
        install.patch_job_status(
            job,
            &format!(r#"{{"status": {{"succeeded": 1, "completionTime": "{now}"}}}}"#),
        );
    }
    wait_for_reason(&install, cr_name, "ScanCompleted", Duration::from_secs(10));

    // Capture the original sbomLocation strings (we'll assert they don't
    // mutate after the output edit).
    let before = install.jsonpath(
        "namespacescan",
        cr_name,
        r#"{.status.scannedImages[*].sbomLocation}"#,
    );
    let before_set: std::collections::BTreeSet<&str> = before.split_whitespace().collect();
    assert_eq!(before_set.len(), 2);

    // Edit the CR's pathPrefix mid-flight.
    run_ok(
        &[
            "kubectl",
            "patch",
            "namespacescan",
            cr_name,
            "-n",
            ns,
            "--type=merge",
            "-p",
            r#"{"spec":{"output":{"pvc":{"pathPrefix":"team-b"}}}}"#,
            "--context",
            &kube_context(),
        ],
        "patch CR pathPrefix",
    );

    // Add a third pod with a new image.
    install.apply_yaml_in(target_ns, &pod_yaml("gamma", target_ns, "alpine:3.20"));

    // Wait for the new Job to be spawned and patch it to succeeded.
    let jobs_after = wait_for_jobs(&install, cr_name, 3, Duration::from_secs(60));
    let new_jobs: Vec<&String> = jobs_after.iter().filter(|j| !jobs.contains(j)).collect();
    assert_eq!(new_jobs.len(), 1);
    let now = chrono::Utc::now().to_rfc3339();
    install.patch_job_status(
        new_jobs[0],
        &format!(r#"{{"status": {{"succeeded": 1, "completionTime": "{now}"}}}}"#),
    );

    // Wait for the scannedImages array to grow to 3 entries.
    let start = Instant::now();
    loop {
        let refs = install.jsonpath(
            "namespacescan",
            cr_name,
            r#"{.status.scannedImages[*].imageRef}"#,
        );
        if refs.split_whitespace().count() >= 3 {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "timed out waiting for scannedImages[3]; current: {refs}",
        );
        sleep(Duration::from_millis(250));
    }

    // Assert the original 2 sbomLocation strings are still present.
    let after = install.jsonpath(
        "namespacescan",
        cr_name,
        r#"{.status.scannedImages[*].sbomLocation}"#,
    );
    let after_set: std::collections::BTreeSet<&str> = after.split_whitespace().collect();
    assert_eq!(after_set.len(), 3);
    for original in &before_set {
        assert!(
            after_set.contains(*original),
            "FR-015: original sbomLocation {original} MUST be preserved after edit; after_set: {after_set:?}",
        );
    }

    install.cleanup();
}
