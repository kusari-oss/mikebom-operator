//! Shared E2E scaffolding: kubectl + helm wrappers.
//!
//! Introduced in feature 008 to support `job_status_feedback.rs` without
//! copy-pasting the full chart-install pattern from `reconciler_skeleton.rs`.
//! `reconciler_skeleton.rs` was NOT modified in feature 008 — its inline
//! helpers stay as-is.

#![allow(dead_code)] // Some helpers are used only by future tests.

use std::process::{Command, Output};
use std::thread::sleep;
use std::time::{Duration, Instant};

pub const CLUSTER_NAME: &str = "mikebom-operator-e2e";
pub const LOCAL_IMAGE_REPO: &str = "mikebom-operator";
pub const LOCAL_IMAGE_TAG: &str = "dev";

pub fn kube_context() -> String {
    format!("kind-{CLUSTER_NAME}")
}

pub fn run(args: &[&str]) -> Output {
    Command::new(args[0])
        .args(&args[1..])
        .output()
        .unwrap_or_else(|err| panic!("failed to spawn `{}`: {err}", args[0]))
}

pub fn run_ok(args: &[&str], context: &str) -> Output {
    let out = run(args);
    assert!(
        out.status.success(),
        "{context}: command failed: {args:?}\nstderr:\n{}\nstdout:\n{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout),
    );
    out
}

pub struct Install {
    pub release: String,
    pub namespace: String,
}

impl Install {
    pub fn new(release: &str, namespace: &str) -> Self {
        Self {
            release: release.to_string(),
            namespace: namespace.to_string(),
        }
    }

    pub fn cleanup(&self) {
        let ctx = kube_context();
        let _ = run(&[
            "helm",
            "uninstall",
            &self.release,
            "-n",
            &self.namespace,
            "--kube-context",
            &ctx,
        ]);
        let _ = run(&[
            "kubectl",
            "delete",
            "namespace",
            &self.namespace,
            "--context",
            &ctx,
            "--ignore-not-found",
            "--wait=false",
        ]);
    }

    pub fn helm_install(&self) {
        let ctx = kube_context();
        run_ok(
            &[
                "helm",
                "install",
                &self.release,
                "charts/mikebom-operator",
                "-n",
                &self.namespace,
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

    pub fn operator_pod_name(&self) -> String {
        let out = run_ok(
            &[
                "kubectl",
                "get",
                "pods",
                "-n",
                &self.namespace,
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

    pub fn apply_yaml_in(&self, target_namespace: &str, yaml: &str) {
        let ctx = kube_context();
        let mut cmd = Command::new("kubectl");
        cmd.args([
            "apply",
            "-n",
            target_namespace,
            "-f",
            "-",
            "--context",
            &ctx,
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
                .expect("failed to write yaml to kubectl stdin");
        }
        let out = child.wait_with_output().expect("kubectl apply failed");
        assert!(
            out.status.success(),
            "kubectl apply -n {target_namespace} failed:\nstderr:\n{}",
            String::from_utf8_lossy(&out.stderr),
        );
    }

    pub fn jsonpath(&self, resource: &str, name: &str, path: &str) -> String {
        let out = run(&[
            "kubectl",
            "get",
            resource,
            name,
            "-n",
            &self.namespace,
            "-o",
            &format!("jsonpath={path}"),
            "--context",
            &kube_context(),
        ]);
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    pub fn list_jobs_for_cr(&self, cr_name: &str) -> Vec<String> {
        let out = run_ok(
            &[
                "kubectl",
                "get",
                "jobs",
                "-n",
                &self.namespace,
                "-l",
                &format!("kusari.dev/namespace-scan={cr_name}"),
                "-o",
                "jsonpath={.items[*].metadata.name}",
                "--context",
                &kube_context(),
            ],
            "list jobs",
        );
        let stdout = String::from_utf8(out.stdout).unwrap();
        stdout.split_whitespace().map(str::to_string).collect()
    }

    /// Patch a Job's `.status` subresource with the provided merge-patch JSON.
    pub fn patch_job_status(&self, job_name: &str, patch_json: &str) {
        run_ok(
            &[
                "kubectl",
                "patch",
                "job",
                job_name,
                "-n",
                &self.namespace,
                "--subresource=status",
                "--type=merge",
                "-p",
                patch_json,
                "--context",
                &kube_context(),
            ],
            "patch job status",
        );
    }
}

pub fn wait_until<F: Fn() -> bool>(
    timeout: Duration,
    interval: Duration,
    label: &str,
    condition: F,
) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if condition() {
            return;
        }
        sleep(interval);
    }
    panic!("timed out after {timeout:?} waiting for: {label}");
}
