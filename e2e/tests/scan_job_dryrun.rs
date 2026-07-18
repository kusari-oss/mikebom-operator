//! Constitution VI E2E for feature 003: asserts the Job manifest produced by
//! `operator::scan_job::build_scan_job` is accepted by a real Kubernetes API
//! server via `kubectl apply --dry-run=server`. No pods are actually scheduled.
//!
//! Gated behind `MIKEBOM_OPERATOR_E2E=1`. Prerequisites: `kind create cluster
//! --config e2e/kind-cluster.yaml`. No image build / load needed — this test
//! validates manifest structure only.

use std::io::Write as _;
use std::process::{Command, Stdio};

use operator::crds::namespace_scan::{
    NamespaceScanSpec, Output, OutputType, ScanFormat, Schedule, Target,
};
use operator::scan_job::{build_scan_job, BuildScanJobError};

const CLUSTER_NAME: &str = "mikebom-operator-e2e";

fn kube_context() -> String {
    format!("kind-{CLUSTER_NAME}")
}

fn valid_spec(format: ScanFormat) -> NamespaceScanSpec {
    NamespaceScanSpec {
        target: Target {
            namespaces: vec!["default".to_string()],
            kinds: vec![],
            label_selector: None,
        },
        schedule: Schedule {
            cron: Some("0 */6 * * *".to_string()),
            interval: None,
        },
        mikebom_image: "ghcr.io/kusari-oss/mikebom:v0.1.0-alpha.64".to_string(),
        scan_format: format,
        output: Output {
            backend_type: OutputType::Pvc,
            pvc: None,
            s3: None,
            oci: None,
        },
    }
}

fn kubectl_dry_run_apply(yaml: &str) {
    let mut child = Command::new("kubectl")
        .args([
            "apply",
            "--dry-run=server",
            "-f",
            "-",
            "-n",
            "default",
            "--context",
            &kube_context(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn kubectl");
    {
        let stdin = child.stdin.as_mut().expect("kubectl stdin");
        stdin.write_all(yaml.as_bytes()).expect("write yaml");
    }
    let out = child.wait_with_output().expect("kubectl wait");
    assert!(
        out.status.success(),
        "kubectl apply --dry-run=server failed:\n--- yaml ---\n{yaml}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&out.stderr),
    );
}

#[test]
fn scan_job_passes_server_dry_run() {
    if std::env::var("MIKEBOM_OPERATOR_E2E").ok().as_deref() != Some("1") {
        eprintln!("MIKEBOM_OPERATOR_E2E unset; skipping kind-based dry-run E2E.");
        return;
    }

    for variant in [
        ScanFormat::CyclonedxJson,
        ScanFormat::Spdx23Json,
        ScanFormat::Spdx3Json,
    ] {
        let spec = valid_spec(variant);
        let job = build_scan_job(&spec, "scan-prod", "nginx:1.27.0")
            .expect("builder should succeed for valid inputs");
        let yaml = serde_yaml::to_string(&job).expect("YAML serialize");
        kubectl_dry_run_apply(&yaml);
    }
}

#[test]
fn empty_mikebom_image_returns_error_path() {
    // No env-gating: this is a pure-Rust assertion, no kind required.
    let mut spec = valid_spec(ScanFormat::CyclonedxJson);
    spec.mikebom_image = "".to_string();
    let err = build_scan_job(&spec, "scan-prod", "nginx:1.27.0").unwrap_err();
    assert_eq!(err, BuildScanJobError::EmptyMikebomImage);
}

fn valid_pvc_spec(format: ScanFormat) -> NamespaceScanSpec {
    use operator::crds::namespace_scan::PvcOutput;
    let mut spec = valid_spec(format);
    spec.output = Output {
        backend_type: OutputType::Pvc,
        pvc: Some(PvcOutput {
            claim_name: "sbom-scratch".to_string(),
            path_prefix: Some("team-a".to_string()),
        }),
        s3: None,
        oci: None,
    };
    spec
}

#[test]
fn pvc_scan_job_passes_server_dry_run() {
    if std::env::var("MIKEBOM_OPERATOR_E2E").ok().as_deref() != Some("1") {
        eprintln!("MIKEBOM_OPERATOR_E2E unset; skipping kind-based dry-run E2E (PVC).");
        return;
    }

    for variant in [
        ScanFormat::CyclonedxJson,
        ScanFormat::Spdx23Json,
        ScanFormat::Spdx3Json,
    ] {
        let spec = valid_pvc_spec(variant);
        let job = build_scan_job(&spec, "scan-prod", "nginx:1.27.0")
            .expect("PVC builder should succeed for valid inputs");
        let yaml = serde_yaml::to_string(&job).expect("YAML serialize");
        kubectl_dry_run_apply(&yaml);
    }
}

fn valid_s3_spec(format: ScanFormat) -> NamespaceScanSpec {
    use operator::crds::namespace_scan::S3Output;
    let mut spec = valid_spec(format);
    spec.output = Output {
        backend_type: OutputType::S3,
        pvc: None,
        s3: Some(S3Output {
            bucket: "sboms-prod".to_string(),
            region: "us-west-2".to_string(),
            path_prefix: Some("team-a".to_string()),
            credentials_secret_name: Some("aws-creds".to_string()),
        }),
        oci: None,
    };
    spec
}

#[test]
fn s3_scan_job_passes_server_dry_run() {
    if std::env::var("MIKEBOM_OPERATOR_E2E").ok().as_deref() != Some("1") {
        eprintln!("MIKEBOM_OPERATOR_E2E unset; skipping kind-based dry-run E2E (S3).");
        return;
    }

    for variant in [
        ScanFormat::CyclonedxJson,
        ScanFormat::Spdx23Json,
        ScanFormat::Spdx3Json,
    ] {
        let spec = valid_s3_spec(variant);
        let job = build_scan_job(&spec, "scan-prod", "nginx:1.27.0")
            .expect("S3 builder should succeed for valid inputs");
        let yaml = serde_yaml::to_string(&job).expect("YAML serialize");
        // Note: kubectl --dry-run=server validates the manifest references
        // syntactically. The aws-creds Secret doesn't need to exist; the API
        // server doesn't check Secret existence for envFrom at dry-run time.
        kubectl_dry_run_apply(&yaml);
    }
}

fn valid_oci_spec(format: ScanFormat) -> NamespaceScanSpec {
    use operator::crds::namespace_scan::OciOutput;
    let mut spec = valid_spec(format);
    spec.output = Output {
        backend_type: OutputType::Oci,
        pvc: None,
        s3: None,
        oci: Some(OciOutput {
            registry: "ghcr.io".to_string(),
            repository: "kusari-oss/sboms".to_string(),
            credentials_secret_name: Some("registry-creds".to_string()),
        }),
    };
    spec
}

#[test]
fn oci_scan_job_passes_server_dry_run() {
    if std::env::var("MIKEBOM_OPERATOR_E2E").ok().as_deref() != Some("1") {
        eprintln!("MIKEBOM_OPERATOR_E2E unset; skipping kind-based dry-run E2E (OCI).");
        return;
    }

    for variant in [
        ScanFormat::CyclonedxJson,
        ScanFormat::Spdx23Json,
        ScanFormat::Spdx3Json,
    ] {
        let spec = valid_oci_spec(variant);
        let job = build_scan_job(&spec, "scan-prod", "nginx:1.27.0")
            .expect("OCI builder should succeed for valid inputs");
        let yaml = serde_yaml::to_string(&job).expect("YAML serialize");
        // The registry-creds Secret of type dockerconfigjson does not need to
        // exist for kubectl --dry-run=server; volume mount references are
        // validated syntactically only.
        kubectl_dry_run_apply(&yaml);
    }
}
