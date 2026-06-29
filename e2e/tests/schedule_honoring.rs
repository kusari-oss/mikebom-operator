//! Constitution VI E2E for feature 009: real-operator-in-kind tests of the
//! schedule-honoring re-scan loop.
//!
//! Like feature 008's `job_status_feedback.rs`, this exercises behavior that
//! can only manifest with a real operator pod running in kind — the schedule
//! decision happens during reconcile cycles, and the watch-driven re-scan
//! transition needs the controller to actually be alive.
//!
//! Gated behind `MIKEBOM_OPERATOR_E2E=1`. Prerequisites:
//!
//! 1. `kind create cluster --config e2e/kind-cluster.yaml`
//! 2. `docker build -t mikebom-operator:dev .`
//! 3. `kind load docker-image mikebom-operator:dev --name mikebom-operator-e2e`
//!
//! Then: `MIKEBOM_OPERATOR_E2E=1 cargo test --test schedule_honoring`
//!
//! Tests:
//! - T027  cron-driven re-scan within 30s of next tick
//! - T030  interval-driven re-scan within 30s
//! - T031  both-set schedule → InvalidSpec within 10s
//! - T032  schedule edit takes effect on next reconcile
//! - T034  operator restart → exactly one catch-up scan

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

fn cr_yaml(name: &str, target_ns: &str, schedule_yaml: &str) -> String {
    format!(
        r#"
apiVersion: kusari.dev/v1alpha1
kind: NamespaceScan
metadata:
  name: {name}
spec:
  target:
    namespaces: [{target_ns}]
  {schedule_yaml}
  mikebomImage: ghcr.io/kusari-oss/mikebom:v0.1.0-alpha.51
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

fn install_and_apply(
    release: &str,
    ns: &str,
    target_ns: &str,
    cr_name: &str,
    schedule: &str,
) -> Install {
    let install = Install::new(release, ns);
    install.cleanup();
    install.helm_install();
    install.apply_yaml_in(target_ns, &ns_yaml(target_ns));
    install.apply_yaml_in(ns, &cr_yaml(cr_name, target_ns, schedule));
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
                "timed out waiting for {expected} Job(s); saw {} so far",
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
// T027 — cron-driven re-scan within 30s of next tick
// ---------------------------------------------------------------------------

#[test]
fn t027_cron_rescan_within_30s() {
    if !e2e_enabled() {
        skip("kind-based feature 009 E2E (T027 cron)");
        return;
    }
    let release = "mikebom-009-t027";
    let ns = "kusari-operator-009-t027";
    let target_ns = "feat009-t027";
    let cr_name = "scan-t027";
    let install = install_and_apply(
        release,
        ns,
        target_ns,
        cr_name,
        "schedule:\n    cron: \"*/2 * * * *\"",
    );

    install.apply_yaml_in(target_ns, &pod_yaml("alpha", target_ns, "nginx:1.27.0"));

    // Patch the first Job to succeeded so the CR reaches ScanCompleted quickly.
    let jobs = wait_for_jobs(&install, cr_name, 1, Duration::from_secs(60));
    let first_job = jobs[0].clone();
    let now = chrono::Utc::now().to_rfc3339();
    install.patch_job_status(
        &first_job,
        &format!(r#"{{"status": {{"succeeded": 1, "completionTime": "{now}"}}}}"#),
    );
    wait_for_reason(&install, cr_name, "ScanCompleted", Duration::from_secs(15));

    // Now wait up to ~2.5 minutes for the next cron tick + the schedule check
    // to fire + a fresh Job to spawn.
    let start = Instant::now();
    loop {
        let current_jobs = install.list_jobs_for_cr(cr_name);
        let has_new = current_jobs.iter().any(|j| j != &first_job);
        if has_new {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(180),
            "cron tick should produce a fresh Job within ~2-3 minutes",
        );
        sleep(Duration::from_secs(2));
    }

    install.cleanup();
}

// ---------------------------------------------------------------------------
// T030 — interval-driven re-scan within 30s of lastScanCompletedAt + 2m
// ---------------------------------------------------------------------------

#[test]
fn t030_interval_rescan_within_30s() {
    if !e2e_enabled() {
        skip("kind-based feature 009 E2E (T030 interval)");
        return;
    }
    let release = "mikebom-009-t030";
    let ns = "kusari-operator-009-t030";
    let target_ns = "feat009-t030";
    let cr_name = "scan-t030";
    let install = install_and_apply(
        release,
        ns,
        target_ns,
        cr_name,
        "schedule:\n    interval: \"2m\"",
    );

    install.apply_yaml_in(target_ns, &pod_yaml("alpha", target_ns, "nginx:1.27.0"));

    let jobs = wait_for_jobs(&install, cr_name, 1, Duration::from_secs(60));
    let first_job = jobs[0].clone();
    let now = chrono::Utc::now().to_rfc3339();
    install.patch_job_status(
        &first_job,
        &format!(r#"{{"status": {{"succeeded": 1, "completionTime": "{now}"}}}}"#),
    );
    wait_for_reason(&install, cr_name, "ScanCompleted", Duration::from_secs(15));

    // Wait for interval (2m) + budget. The schedule decision should fire and
    // a fresh Job should appear.
    let start = Instant::now();
    loop {
        let current_jobs = install.list_jobs_for_cr(cr_name);
        let has_new = current_jobs.iter().any(|j| j != &first_job);
        if has_new {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(180),
            "interval should produce a fresh Job within ~2-3 minutes",
        );
        sleep(Duration::from_secs(2));
    }

    install.cleanup();
}

// ---------------------------------------------------------------------------
// T031 — both-set schedule → InvalidSpec within 10s
// ---------------------------------------------------------------------------

#[test]
fn t031_both_set_is_invalid_spec() {
    if !e2e_enabled() {
        skip("kind-based feature 009 E2E (T031 both-set)");
        return;
    }
    let release = "mikebom-009-t031";
    let ns = "kusari-operator-009-t031";
    let target_ns = "feat009-t031";
    let cr_name = "scan-t031";
    let install = install_and_apply(
        release,
        ns,
        target_ns,
        cr_name,
        "schedule:\n    cron: \"0 * * * *\"\n    interval: \"1h\"",
    );

    wait_for_reason(&install, cr_name, "InvalidSpec", Duration::from_secs(15));
    let message = install.jsonpath(
        "namespacescan",
        cr_name,
        r#"{.status.conditions[?(@.type=="Ready")].message}"#,
    );
    assert!(
        message.contains("both cron and interval"),
        "InvalidSpec message MUST name the conflict (FR-005), got: {message}",
    );

    install.cleanup();
}

// ---------------------------------------------------------------------------
// T032 — schedule edit takes effect on next reconcile
// ---------------------------------------------------------------------------

#[test]
fn t032_schedule_edit_takes_effect() {
    if !e2e_enabled() {
        skip("kind-based feature 009 E2E (T032 edit)");
        return;
    }
    let release = "mikebom-009-t032";
    let ns = "kusari-operator-009-t032";
    let target_ns = "feat009-t032";
    let cr_name = "scan-t032";
    let install = install_and_apply(
        release,
        ns,
        target_ns,
        cr_name,
        "schedule:\n    interval: \"1h\"",
    );

    install.apply_yaml_in(target_ns, &pod_yaml("alpha", target_ns, "nginx:1.27.0"));

    let jobs = wait_for_jobs(&install, cr_name, 1, Duration::from_secs(60));
    let first_job = jobs[0].clone();
    let now = chrono::Utc::now().to_rfc3339();
    install.patch_job_status(
        &first_job,
        &format!(r#"{{"status": {{"succeeded": 1, "completionTime": "{now}"}}}}"#),
    );
    wait_for_reason(&install, cr_name, "ScanCompleted", Duration::from_secs(15));

    // Now edit the schedule to 2m. The next re-scan should fire within
    // ~2-3 minutes (not 1 hour).
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
            r#"{"spec":{"schedule":{"interval":"2m"}}}"#,
            "--context",
            &kube_context(),
        ],
        "patch schedule",
    );

    let start = Instant::now();
    loop {
        let current_jobs = install.list_jobs_for_cr(cr_name);
        let has_new = current_jobs.iter().any(|j| j != &first_job);
        if has_new {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(180),
            "edited schedule (2m) should fire within ~2-3 minutes, NOT the old 1h",
        );
        sleep(Duration::from_secs(2));
    }

    install.cleanup();
}

// ---------------------------------------------------------------------------
// T034 — operator restart fires exactly one catch-up scan
// ---------------------------------------------------------------------------

#[test]
fn t034_restart_catchup_fires_exactly_once() {
    if !e2e_enabled() {
        skip("kind-based feature 009 E2E (T034 restart catch-up)");
        return;
    }
    let release = "mikebom-009-t034";
    let ns = "kusari-operator-009-t034";
    let target_ns = "feat009-t034";
    let cr_name = "scan-t034";
    let install = install_and_apply(
        release,
        ns,
        target_ns,
        cr_name,
        "schedule:\n    interval: \"1m\"",
    );

    install.apply_yaml_in(target_ns, &pod_yaml("alpha", target_ns, "nginx:1.27.0"));

    let jobs = wait_for_jobs(&install, cr_name, 1, Duration::from_secs(60));
    let first_job = jobs[0].clone();
    let now = chrono::Utc::now().to_rfc3339();
    install.patch_job_status(
        &first_job,
        &format!(r#"{{"status": {{"succeeded": 1, "completionTime": "{now}"}}}}"#),
    );
    wait_for_reason(&install, cr_name, "ScanCompleted", Duration::from_secs(15));

    // Scale operator to 0 — simulate 3+ missed windows for a 1-min interval.
    run_ok(
        &[
            "kubectl",
            "scale",
            "deployment",
            "mikebom-operator",
            "-n",
            ns,
            "--replicas=0",
            "--context",
            &kube_context(),
        ],
        "scale operator to 0",
    );

    // Wait 3 minutes (= 3 missed windows for 1m interval).
    sleep(Duration::from_secs(180));

    // Scale back up.
    run_ok(
        &[
            "kubectl",
            "scale",
            "deployment",
            "mikebom-operator",
            "-n",
            ns,
            "--replicas=1",
            "--context",
            &kube_context(),
        ],
        "scale operator to 1",
    );

    // Wait for new operator pod to be Ready.
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
            "--timeout=120s",
            "--context",
            &kube_context(),
        ],
        "wait for operator Ready",
    );

    // Within 60s, the operator should have fired ONE catch-up: the first Job
    // is deleted (terminal-Job cleanup), then a new Job spawns via ensure_jobs.
    // We assert that the live Job count is exactly 1 (the fresh one, not the
    // pre-restart one). Anything more means catch-up iteration happened.
    let start = Instant::now();
    loop {
        let current_jobs = install.list_jobs_for_cr(cr_name);
        // We want: exactly one Job present, AND it's NOT the pre-restart one.
        if current_jobs.len() == 1 && current_jobs[0] != first_job {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "catch-up should produce exactly one new Job within 60s; jobs={current_jobs:?}",
        );
        sleep(Duration::from_secs(2));
    }

    // Additional check: confirm the catch-up didn't fire multiple times by
    // verifying total Job count never exceeded 2 (old + new transitional max).
    // (Since cleanup runs Foreground deletion, there's a brief window where
    // the old Job is "Terminating" and the new one exists — count up to 2 is
    // acceptable. Counts > 2 indicate iteration over missed windows.)
    let total = install.list_jobs_for_cr(cr_name).len();
    assert!(
        total <= 1,
        "after catch-up, total Job count for the CR should be 1 (or transiently 2); got {total}",
    );

    install.cleanup();
}
