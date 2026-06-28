//! Pure builder for the 3-container scan Job.
//!
//! Constructs a `batch/v1.Job` from a `NamespaceScanSpec` + image ref per
//! `specs/003-scan-job-builder/contracts/build-scan-job.md`. No I/O, no
//! reconciler integration — feature 004+ wires this into the controller
//! and replaces the `output-upload` container with concrete backend code.
//!
//! Container partition (per data-model.md §2):
//!   initContainers = [init-pull, mikebom-scan]   // run sequentially
//!   containers     = [output-upload]             // runs last; terminates pod

use std::collections::BTreeMap;

use k8s_openapi::api::batch::v1::{Job, JobSpec};
use k8s_openapi::api::core::v1::{
    Container, EmptyDirVolumeSource, EnvVar, PodSpec, PodTemplateSpec, ResourceRequirements,
    Volume, VolumeMount,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::crds::namespace_scan::{NamespaceScanSpec, ScanFormat};

/// Hardcoded defaults for the v0.3 builder. Pinned to manifest digests per
/// feature 001's constitution-VII pinning convention.
pub mod defaults {
    /// Init-pull image — extracts the target image's rootfs via `crane export`.
    ///
    /// Distroless-with-busybox base (ships `sh` + `tar` + `crane`). `crane export <ref> -`
    /// produces a flat tarball directly, replacing the skopeo + per-layer `tar -x` loop
    /// from research §R1's original plan (Chainguard's `:latest` skopeo is now gated by
    /// the paid Chainguard Direct subscription). Maintained upstream by Google's
    /// go-containerregistry project.
    ///
    /// Refresh via `crane digest gcr.io/go-containerregistry/crane:debug`.
    pub const INIT_PULL_IMAGE: &str =
        "gcr.io/go-containerregistry/crane@sha256:1b1fb24d2b1bb27a9daf81a588157e68463876904e8e537a812edba6284fb252";
    // crane:debug latest as of 2026-06-28

    /// Output-upload v0.3 placeholder: Chainguard's free-tier distroless busybox.
    /// Replaced by features 004/005/006 with concrete PVC / S3 / OCI backend wiring.
    pub const OUTPUT_UPLOAD_IMAGE: &str =
        "cgr.dev/chainguard/busybox@sha256:accc5c911abaf2f70487f93cad07b0891d502cbba7e79f96d1db9074ef40928a";
    // latest as of 2026-06-28

    pub const TTL_SECONDS_AFTER_FINISHED: i32 = 3600;
    pub const BACKOFF_LIMIT: i32 = 2;
    pub const SCAN_CPU_REQUEST: &str = "100m";
    pub const SCAN_MEMORY_REQUEST: &str = "128Mi";
}

/// Failure modes for `build_scan_job`. Two narrow cases caught before
/// constructing a malformed Job.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum BuildScanJobError {
    #[error("spec.mikebomImage is empty or whitespace-only")]
    EmptyMikebomImage,

    #[error("image_ref is empty or whitespace-only")]
    EmptyImageRef,
}

/// Build a 3-container scan Job for the given `NamespaceScan` CR and target image ref.
///
/// See `specs/003-scan-job-builder/contracts/build-scan-job.md` for the
/// stable contract callers can rely on.
pub fn build_scan_job(
    spec: &NamespaceScanSpec,
    cr_name: &str,
    image_ref: &str,
) -> Result<Job, BuildScanJobError> {
    let mikebom_image = spec.mikebom_image.trim();
    if mikebom_image.is_empty() {
        return Err(BuildScanJobError::EmptyMikebomImage);
    }
    let image_ref = image_ref.trim();
    if image_ref.is_empty() {
        return Err(BuildScanJobError::EmptyImageRef);
    }

    let short_hash = short_image_hash(image_ref);
    let name = job_name(cr_name, &short_hash);

    let labels = BTreeMap::from([
        (
            "app.kubernetes.io/name".to_string(),
            "mikebom-operator".to_string(),
        ),
        (
            "app.kubernetes.io/component".to_string(),
            "scan-job".to_string(),
        ),
        ("kusari.dev/namespace-scan".to_string(), cr_name.to_string()),
        ("kusari.dev/image-ref-hash".to_string(), short_hash.clone()),
    ]);

    Ok(Job {
        metadata: ObjectMeta {
            name: Some(name),
            labels: Some(labels.clone()),
            ..Default::default()
        },
        spec: Some(JobSpec {
            completions: Some(1),
            parallelism: Some(1),
            backoff_limit: Some(defaults::BACKOFF_LIMIT),
            ttl_seconds_after_finished: Some(defaults::TTL_SECONDS_AFTER_FINISHED),
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(labels),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    restart_policy: Some("Never".to_string()),
                    volumes: Some(vec![workdir_volume()]),
                    init_containers: Some(vec![
                        build_init_pull_container(image_ref),
                        build_mikebom_scan_container(spec, &short_hash, mikebom_image),
                    ]),
                    containers: vec![build_output_upload_container()],
                    ..Default::default()
                }),
            },
            ..Default::default()
        }),
        ..Default::default()
    })
}

/// 7-char SHA-256 prefix of the image ref. Provides Job-name uniqueness without
/// trying to DNS-1123-sanitize digest-pinned refs.
fn short_image_hash(image_ref: &str) -> String {
    let digest = Sha256::digest(image_ref.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    hex.chars().take(7).collect()
}

/// `nsscan-<sanitized-cr-name>-<7-char-hash>`, capped at 63 chars.
fn job_name(cr_name: &str, short_hash: &str) -> String {
    let sanitized = sanitize_dns1123(cr_name);
    // Reserve room for the static prefix "nsscan-" (7), the hyphen + hash (8), total 15.
    let max_sanitized_len = 63usize.saturating_sub(15);
    let sanitized = if sanitized.len() > max_sanitized_len {
        sanitized
            .chars()
            .take(max_sanitized_len)
            .collect::<String>()
    } else {
        sanitized
    };
    let sanitized = sanitized.trim_end_matches('-').to_string();
    format!("nsscan-{sanitized}-{short_hash}")
}

/// Replace non-`[a-z0-9-]` chars with `-`, collapse runs, lowercase.
fn sanitize_dns1123(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_was_hyphen = false;
    for ch in s.chars() {
        let mapped = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else {
            '-'
        };
        if mapped == '-' {
            if !last_was_hyphen && !out.is_empty() {
                out.push('-');
                last_was_hyphen = true;
            }
        } else {
            out.push(mapped);
            last_was_hyphen = false;
        }
    }
    out.trim_matches('-').to_string()
}

fn workdir_volume() -> Volume {
    Volume {
        name: "workdir".to_string(),
        empty_dir: Some(EmptyDirVolumeSource::default()),
        ..Default::default()
    }
}

fn workdir_mount() -> VolumeMount {
    VolumeMount {
        name: "workdir".to_string(),
        mount_path: "/workdir".to_string(),
        ..Default::default()
    }
}

fn build_init_pull_container(image_ref: &str) -> Container {
    // `crane export` walks the image's layer composition (including whiteouts) and emits
    // a flat tarball on stdout — one pipeline step, no per-layer iteration needed.
    let script = "set -eu\n\
        mkdir -p /workdir/rootfs /workdir/out\n\
        crane export \"$IMAGE_REF\" - | tar -x -C /workdir/rootfs";
    Container {
        name: "init-pull".to_string(),
        image: Some(defaults::INIT_PULL_IMAGE.to_string()),
        command: Some(vec!["sh".to_string(), "-c".to_string(), script.to_string()]),
        env: Some(vec![EnvVar {
            name: "IMAGE_REF".to_string(),
            value: Some(image_ref.to_string()),
            ..Default::default()
        }]),
        volume_mounts: Some(vec![workdir_mount()]),
        ..Default::default()
    }
}

fn build_mikebom_scan_container(
    spec: &NamespaceScanSpec,
    short_hash: &str,
    mikebom_image: &str,
) -> Container {
    let (format_arg, extension) = scan_format_args(&spec.scan_format);
    let output_file = format!("/workdir/out/{short_hash}.{extension}");
    let args = vec![
        "sbom".to_string(),
        "scan".to_string(),
        "--path".to_string(),
        "/workdir/rootfs".to_string(),
        "--format".to_string(),
        format_arg.to_string(),
        "--output".to_string(),
        format!("{format_arg}={output_file}"),
    ];

    let requests = BTreeMap::from([
        (
            "cpu".to_string(),
            Quantity(defaults::SCAN_CPU_REQUEST.to_string()),
        ),
        (
            "memory".to_string(),
            Quantity(defaults::SCAN_MEMORY_REQUEST.to_string()),
        ),
    ]);

    Container {
        name: "mikebom-scan".to_string(),
        image: Some(mikebom_image.to_string()),
        args: Some(args),
        resources: Some(ResourceRequirements {
            requests: Some(requests),
            ..Default::default()
        }),
        volume_mounts: Some(vec![workdir_mount()]),
        ..Default::default()
    }
}

fn build_output_upload_container() -> Container {
    Container {
        name: "output-upload".to_string(),
        image: Some(defaults::OUTPUT_UPLOAD_IMAGE.to_string()),
        command: Some(vec![
            "sh".to_string(),
            "-c".to_string(),
            "ls -la /workdir/out/ && cat /workdir/out/*.json".to_string(),
        ]),
        volume_mounts: Some(vec![workdir_mount()]),
        ..Default::default()
    }
}

fn scan_format_args(format: &ScanFormat) -> (&'static str, &'static str) {
    match format {
        ScanFormat::CyclonedxJson => ("cyclonedx-json", "cdx.json"),
        ScanFormat::Spdx23Json => ("spdx-2.3-json", "spdx.json"),
        ScanFormat::Spdx3Json => ("spdx-3-json", "spdx3.json"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crds::namespace_scan::{Output, OutputType, Schedule, Target};
    use regex::Regex;

    fn valid_spec() -> NamespaceScanSpec {
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
            mikebom_image: "ghcr.io/kusari-oss/mikebom:v0.1.0-alpha.51".to_string(),
            scan_format: ScanFormat::CyclonedxJson,
            output: Output {
                backend_type: OutputType::Pvc,
                pvc: None,
                s3: None,
                oci: None,
            },
        }
    }

    // ---------- US1: builder produces a valid Job manifest ----------

    #[test]
    fn name_is_dns1123_compliant() {
        let job = build_scan_job(&valid_spec(), "scan-prod", "nginx:1.27.0")
            .expect("builder should succeed");
        let name = job.metadata.name.as_deref().expect("name must be set");
        let re = Regex::new(r"^[a-z0-9]([-a-z0-9]*[a-z0-9])?$").unwrap();
        assert!(re.is_match(name), "name {name:?} is not DNS-1123 compliant");
        assert!(name.len() <= 63, "name {name:?} exceeds 63 char limit");
    }

    #[test]
    fn name_is_deterministic() {
        let a = build_scan_job(&valid_spec(), "scan-prod", "nginx:1.27.0").unwrap();
        let b = build_scan_job(&valid_spec(), "scan-prod", "nginx:1.27.0").unwrap();
        assert_eq!(a.metadata.name, b.metadata.name);
    }

    #[test]
    fn name_differs_for_different_images() {
        let a = build_scan_job(&valid_spec(), "scan-prod", "nginx:1.27.0").unwrap();
        let b = build_scan_job(&valid_spec(), "scan-prod", "nginx:1.27.1").unwrap();
        assert_ne!(a.metadata.name, b.metadata.name);
    }

    #[test]
    fn name_truncates_long_cr_name() {
        let long_cr = "a".repeat(100);
        let job = build_scan_job(&valid_spec(), &long_cr, "nginx:1.27.0").unwrap();
        let name = job.metadata.name.as_deref().unwrap();
        assert!(name.len() <= 63, "name {name:?} > 63 chars");
        assert!(name.starts_with("nsscan-"));
    }

    #[test]
    fn empty_mikebom_image_errors() {
        let mut spec = valid_spec();
        spec.mikebom_image = "".to_string();
        let err = build_scan_job(&spec, "scan-prod", "nginx:1.27.0").unwrap_err();
        assert_eq!(err, BuildScanJobError::EmptyMikebomImage);

        spec.mikebom_image = "   ".to_string();
        let err = build_scan_job(&spec, "scan-prod", "nginx:1.27.0").unwrap_err();
        assert_eq!(err, BuildScanJobError::EmptyMikebomImage);
    }

    #[test]
    fn empty_image_ref_errors() {
        let err = build_scan_job(&valid_spec(), "scan-prod", "").unwrap_err();
        assert_eq!(err, BuildScanJobError::EmptyImageRef);
        let err = build_scan_job(&valid_spec(), "scan-prod", "   ").unwrap_err();
        assert_eq!(err, BuildScanJobError::EmptyImageRef);
    }

    // ---------- US2: 3-container choreography ----------

    fn pod_spec(job: &Job) -> &PodSpec {
        job.spec
            .as_ref()
            .expect("job.spec")
            .template
            .spec
            .as_ref()
            .expect("template.spec")
    }

    #[test]
    fn pod_template_has_three_containers_in_correct_order() {
        let job = build_scan_job(&valid_spec(), "scan-prod", "nginx:1.27.0").unwrap();
        let ps = pod_spec(&job);
        let inits = ps.init_containers.as_ref().expect("init_containers");
        assert_eq!(inits.len(), 2, "expected exactly 2 init containers");
        assert_eq!(inits[0].name, "init-pull");
        assert_eq!(inits[1].name, "mikebom-scan");
        assert_eq!(ps.containers.len(), 1, "expected exactly 1 main container");
        assert_eq!(ps.containers[0].name, "output-upload");
    }

    #[test]
    fn all_containers_share_workdir_emptydir() {
        let job = build_scan_job(&valid_spec(), "scan-prod", "nginx:1.27.0").unwrap();
        let ps = pod_spec(&job);
        let volumes = ps.volumes.as_ref().expect("volumes");
        let workdir = volumes
            .iter()
            .find(|v| v.name == "workdir")
            .expect("workdir volume must exist");
        assert!(workdir.empty_dir.is_some(), "workdir must be an emptyDir");

        let assert_mounted = |c: &Container| {
            let mounts = c.volume_mounts.as_ref().expect("volume_mounts");
            assert!(
                mounts
                    .iter()
                    .any(|m| m.name == "workdir" && m.mount_path == "/workdir"),
                "container {} does not mount workdir at /workdir",
                c.name,
            );
        };
        for c in ps.init_containers.as_ref().unwrap() {
            assert_mounted(c);
        }
        for c in &ps.containers {
            assert_mounted(c);
        }
    }

    #[test]
    fn init_pull_extracts_rootfs() {
        let job = build_scan_job(&valid_spec(), "scan-prod", "nginx:1.27.0").unwrap();
        let init = &pod_spec(&job).init_containers.as_ref().unwrap()[0];
        assert_eq!(init.image.as_deref(), Some(defaults::INIT_PULL_IMAGE));
        let cmd = init.command.as_ref().unwrap().join(" ");
        assert!(
            cmd.contains("crane export"),
            "init-pull missing crane export: {cmd}"
        );
        assert!(
            cmd.contains("/workdir/rootfs"),
            "init-pull missing rootfs target: {cmd}"
        );
        let env = init.env.as_ref().unwrap();
        assert!(
            env.iter()
                .any(|e| e.name == "IMAGE_REF" && e.value.as_deref() == Some("nginx:1.27.0")),
            "init-pull missing IMAGE_REF env",
        );
    }

    #[test]
    fn mikebom_scan_uses_spec_image_and_args() {
        let job = build_scan_job(&valid_spec(), "scan-prod", "nginx:1.27.0").unwrap();
        let scan = &pod_spec(&job).init_containers.as_ref().unwrap()[1];
        assert_eq!(
            scan.image.as_deref(),
            Some("ghcr.io/kusari-oss/mikebom:v0.1.0-alpha.51"),
        );
        let args = scan.args.as_ref().unwrap();
        assert_eq!(&args[0..4], &["sbom", "scan", "--path", "/workdir/rootfs"]);
        assert!(args.iter().any(|a| a == "--format"));
        assert!(args.iter().any(|a| a == "--output"));
    }

    #[test]
    fn mikebom_scan_format_branches() {
        for (variant, expected_format, expected_ext) in [
            (ScanFormat::CyclonedxJson, "cyclonedx-json", "cdx.json"),
            (ScanFormat::Spdx23Json, "spdx-2.3-json", "spdx.json"),
            (ScanFormat::Spdx3Json, "spdx-3-json", "spdx3.json"),
        ] {
            let mut spec = valid_spec();
            spec.scan_format = variant;
            let job = build_scan_job(&spec, "scan-prod", "nginx:1.27.0").unwrap();
            let args = pod_spec(&job).init_containers.as_ref().unwrap()[1]
                .args
                .as_ref()
                .unwrap();
            assert!(
                args.iter().any(|a| a == expected_format),
                "args missing --format value {expected_format}: {args:?}",
            );
            assert!(
                args.iter().any(|a| a.ends_with(expected_ext)),
                "args missing output file extension .{expected_ext}: {args:?}",
            );
        }
    }

    #[test]
    fn output_upload_is_v03_placeholder() {
        let job = build_scan_job(&valid_spec(), "scan-prod", "nginx:1.27.0").unwrap();
        let upload = &pod_spec(&job).containers[0];
        assert_eq!(upload.image.as_deref(), Some(defaults::OUTPUT_UPLOAD_IMAGE));
        let cmd = upload.command.as_ref().unwrap().join(" ");
        assert!(
            cmd.contains("ls -la /workdir/out/"),
            "output-upload missing ls: {cmd}"
        );
        assert!(
            cmd.contains("cat /workdir/out/*.json"),
            "output-upload missing cat: {cmd}",
        );
    }

    #[test]
    fn all_container_images_are_pinned() {
        let job = build_scan_job(&valid_spec(), "scan-prod", "nginx:1.27.0").unwrap();
        let ps = pod_spec(&job);
        let mut all: Vec<&Container> = ps.init_containers.as_ref().unwrap().iter().collect();
        all.extend(ps.containers.iter());
        for c in all {
            let img = c.image.as_deref().unwrap_or("");
            assert!(!img.is_empty(), "container {} has empty image", c.name);
            assert!(
                !img.ends_with(":latest"),
                "container {} uses :latest tag — must pin",
                c.name,
            );
            let is_digest_pinned = img.contains("@sha256:");
            // Tag-pinned mikebom-scan reaches here via spec.mikebomImage; accept tag OR digest.
            let has_tag = img
                .rsplit_once(':')
                .map(|(_, t)| !t.is_empty() && !t.contains('/'))
                .unwrap_or(false);
            assert!(
                is_digest_pinned || has_tag,
                "container {} image {img} is not tag- or digest-pinned",
                c.name,
            );
        }
    }

    // ---------- US3: Job lifecycle policies ----------

    #[test]
    fn job_lifecycle_policies_are_one_shot() {
        let job = build_scan_job(&valid_spec(), "scan-prod", "nginx:1.27.0").unwrap();
        let js = job.spec.as_ref().unwrap();
        assert_eq!(js.completions, Some(1));
        assert_eq!(js.parallelism, Some(1));
        let bo = js.backoff_limit.expect("backoff_limit");
        assert!(bo <= 3, "backoff_limit {bo} > 3");
        let pod = js.template.spec.as_ref().unwrap();
        assert_eq!(pod.restart_policy.as_deref(), Some("Never"));
    }

    #[test]
    fn ttl_within_one_hour() {
        let job = build_scan_job(&valid_spec(), "scan-prod", "nginx:1.27.0").unwrap();
        let ttl = job
            .spec
            .as_ref()
            .unwrap()
            .ttl_seconds_after_finished
            .unwrap();
        assert!(ttl > 0 && ttl <= 3600, "ttl {ttl} out of (0, 3600]");
    }

    #[test]
    fn mikebom_scan_has_resource_requests() {
        let job = build_scan_job(&valid_spec(), "scan-prod", "nginx:1.27.0").unwrap();
        let scan = &pod_spec(&job).init_containers.as_ref().unwrap()[1];
        let requests = scan
            .resources
            .as_ref()
            .and_then(|r| r.requests.as_ref())
            .expect("mikebom-scan resources.requests");
        assert!(requests.contains_key("cpu"), "missing cpu request");
        assert!(requests.contains_key("memory"), "missing memory request");
    }
}
