//! Pure builder for the 3-container scan Job.
//!
//! Constructs a `batch/v1.Job` from a `NamespaceScanSpec` + image ref per
//! `specs/003-scan-job-builder/contracts/build-scan-job.md`. No I/O, no
//! reconciler integration (still — features 004/005/006 added real output
//! backends but reconciler-spawns-Job wiring is a separate later feature).
//!
//! Container partition (per data-model.md §2):
//!   initContainers = [init-pull, mikebom-scan]   // run sequentially
//!   containers     = [output-upload]             // runs last; terminates pod
//!
//! Output-upload dispatch (see `specs/004-pvc-backend/contracts/output-backends.md`):
//!   OutputType::Pvc → busybox + `cp` to a mounted PVC                 (feature 004)
//!   OutputType::S3  → aws-cli + `aws s3 cp` with envFrom Secret creds (feature 005)
//!   OutputType::Oci → ORAS + `oras push` with dockerconfigjson Secret (feature 006)

use std::collections::BTreeMap;

use k8s_openapi::api::batch::v1::{Job, JobSpec};
use k8s_openapi::api::core::v1::{
    Container, EmptyDirVolumeSource, EnvFromSource, EnvVar, KeyToPath,
    PersistentVolumeClaimVolumeSource, PodSpec, PodTemplateSpec, ResourceRequirements,
    SecretEnvSource, SecretVolumeSource, Volume, VolumeMount,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::crds::namespace_scan::{
    NamespaceScanSpec, OciOutput, OutputType, PvcOutput, S3Output, ScanFormat,
};

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

    /// PVC upload image — Chainguard's free-tier distroless busybox. Ships
    /// `sh + cp + mkdir` for the simple file-copy workflow that PVC needs.
    pub const PVC_UPLOAD_IMAGE: &str =
        "cgr.dev/chainguard/busybox@sha256:accc5c911abaf2f70487f93cad07b0891d502cbba7e79f96d1db9074ef40928a";
    // latest as of 2026-06-28

    /// S3 upload image — AWS-maintained official AWS CLI image (feature 005).
    /// Publicly accessible via Amazon ECR. Ships `aws` + a POSIX `sh` so we can
    /// override the default `aws` entrypoint with `sh -c "..."` for our script.
    ///
    /// Refresh via `crane digest public.ecr.aws/aws-cli/aws-cli:latest`.
    pub const S3_UPLOAD_IMAGE: &str =
        "public.ecr.aws/aws-cli/aws-cli@sha256:749bfaf91d690b9a1768083822d620f96c19defdf9ca2dc227eb3695281fda5b";
    // aws-cli:latest as of 2026-06-28

    /// OCI upload image — official ORAS distroless image (feature 006).
    /// Publicly accessible via GHCR. Has no shell, so we use k8s container-arg
    /// `$(VAR)` substitution rather than a shell script.
    ///
    /// Refresh via `crane digest ghcr.io/oras-project/oras:v1.2.0`.
    pub const OCI_UPLOAD_IMAGE: &str =
        "ghcr.io/oras-project/oras@sha256:0087224dd0decc354b5b0689068fbbc40cd5dc3dbf65fcb3868dfbd363dc790b";
    // oras:v1.2.0 latest as of 2026-06-28

    /// Volume name for the docker-registry credentials Secret mount.
    pub const DOCKER_CONFIG_VOLUME_NAME: &str = "oci-credentials";
    /// Mount path for the docker-registry credentials inside output-upload.
    pub const DOCKER_CONFIG_MOUNT_PATH: &str = "/docker-config";

    pub const TTL_SECONDS_AFTER_FINISHED: i32 = 3600;
    pub const BACKOFF_LIMIT: i32 = 2;
    pub const SCAN_CPU_REQUEST: &str = "100m";
    pub const SCAN_MEMORY_REQUEST: &str = "128Mi";

    /// Pod-spec volume name for the PVC output backend (feature 004).
    pub const PVC_VOLUME_NAME: &str = "pvc-output";

    /// Mount path inside the `output-upload` container where the PVC volume appears.
    pub const PVC_MOUNT_PATH: &str = "/pvc-output";
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

    #[error("spec.output.type=Pvc requires spec.output.pvc.claimName to be non-empty")]
    MissingPvcConfig,

    #[error("spec.output.type=S3 requires spec.output.s3 with non-empty bucket and credentialsSecretName")]
    MissingS3Config,

    #[error("spec.output.type=Oci requires spec.output.oci with non-empty registry, repository, and credentialsSecretName")]
    MissingOciConfig,
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

    // Output-upload container construction may fail (e.g., Pvc + missing claim_name);
    // bubble the error up before we start assembling the rest of the Job.
    let output_upload = build_output_upload_container(spec, &short_hash)?;

    // Pod-spec volumes always include `workdir`; PVC and OCI arms add backend-specific
    // volumes (PVC claim mount / docker-config secret) when wired.
    let mut volumes = vec![workdir_volume()];
    match spec.output.backend_type {
        OutputType::Pvc => {
            if let Some(pvc) = &spec.output.pvc {
                if !pvc.claim_name.trim().is_empty() {
                    volumes.push(pvc_output_volume(&pvc.claim_name));
                }
            }
        }
        OutputType::Oci => {
            if let Some(oci) = &spec.output.oci {
                if let Some(secret) = oci
                    .credentials_secret_name
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                {
                    volumes.push(oci_credentials_volume(secret));
                }
            }
        }
        OutputType::S3 => {}
    }

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
                    volumes: Some(volumes),
                    init_containers: Some(vec![
                        build_init_pull_container(image_ref),
                        build_mikebom_scan_container(spec, &short_hash, mikebom_image),
                    ]),
                    containers: vec![output_upload],
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
///
/// Re-exported `pub(crate)` so feature 007's reconciler can compute the same
/// Job name for its get-before-create idempotency check (FR-004).
pub(crate) fn short_image_hash(image_ref: &str) -> String {
    let digest = Sha256::digest(image_ref.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    hex.chars().take(7).collect()
}

/// `nsscan-<sanitized-cr-name>-<7-char-hash>`, capped at 63 chars.
///
/// Re-exported `pub(crate)` so feature 007's reconciler can compute the same
/// Job name for its get-before-create idempotency check (FR-004).
pub(crate) fn job_name(cr_name: &str, short_hash: &str) -> String {
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

/// Pod-spec `Volume` backed by the user-supplied PVC claim (feature 004).
fn pvc_output_volume(claim_name: &str) -> Volume {
    Volume {
        name: defaults::PVC_VOLUME_NAME.to_string(),
        persistent_volume_claim: Some(PersistentVolumeClaimVolumeSource {
            claim_name: claim_name.to_string(),
            read_only: None,
        }),
        ..Default::default()
    }
}

/// `VolumeMount` for the PVC, mounted only on `output-upload` (FR-007).
fn pvc_output_mount() -> VolumeMount {
    VolumeMount {
        name: defaults::PVC_VOLUME_NAME.to_string(),
        mount_path: defaults::PVC_MOUNT_PATH.to_string(),
        ..Default::default()
    }
}

/// Strip a single leading `/` from `s` per FR-008. Idempotent: `"team-a"` stays
/// `"team-a"`; `"/team-a"` becomes `"team-a"`; `"//team-a"` becomes `"/team-a"`.
fn strip_leading_slash(s: &str) -> &str {
    s.strip_prefix('/').unwrap_or(s)
}

/// Pvc-variant `output-upload` container — copies SBOMs from `/workdir/out/`
/// into the PVC mount path (optionally under `pathPrefix`).
fn build_output_upload_pvc_container(pvc: &PvcOutput) -> Container {
    let path_prefix = pvc
        .path_prefix
        .as_deref()
        .map(strip_leading_slash)
        .unwrap_or("");
    // `set -eu` halts on error; `${PATH_PREFIX:+/${PATH_PREFIX}}` emits `/<value>`
    // when PATH_PREFIX is non-empty, nothing otherwise. Avoids brittle string-trim
    // on a leading-slash boundary.
    let script = "set -eu\n\
        DEST=\"/pvc-output${PATH_PREFIX:+/${PATH_PREFIX}}\"\n\
        mkdir -p \"$DEST\"\n\
        cp /workdir/out/*.json \"$DEST/\"";
    Container {
        name: "output-upload".to_string(),
        image: Some(defaults::PVC_UPLOAD_IMAGE.to_string()),
        command: Some(vec!["sh".to_string(), "-c".to_string(), script.to_string()]),
        env: Some(vec![EnvVar {
            name: "PATH_PREFIX".to_string(),
            value: Some(path_prefix.to_string()),
            ..Default::default()
        }]),
        volume_mounts: Some(vec![workdir_mount(), pvc_output_mount()]),
        ..Default::default()
    }
}

/// S3-variant `output-upload` container — copies SBOMs to an S3 bucket via
/// `aws s3 cp`. AWS credentials are sourced from a user-supplied Secret via
/// `envFrom: { secretRef }`; region and bucket are literal env vars from the
/// spec.
fn build_output_upload_s3_container(s3: &S3Output) -> Container {
    let path_prefix = s3
        .path_prefix
        .as_deref()
        .map(strip_leading_slash)
        .unwrap_or("");
    // Mirrors the PVC arm's POSIX `${X:+/${X}}` trick for clean empty-prefix
    // handling. `--recursive --include` keeps the upload scoped to SBOM JSON.
    let script = "set -eu\n\
        DEST=\"s3://$S3_BUCKET${S3_PATH_PREFIX:+/${S3_PATH_PREFIX}}\"\n\
        aws s3 cp /workdir/out/ \"$DEST/\" --recursive --exclude '*' --include '*.json'";

    let credentials_secret = s3
        .credentials_secret_name
        .as_deref()
        .expect("validated upstream by dispatch")
        .to_string();

    Container {
        name: "output-upload".to_string(),
        image: Some(defaults::S3_UPLOAD_IMAGE.to_string()),
        command: Some(vec!["sh".to_string(), "-c".to_string(), script.to_string()]),
        env: Some(vec![
            EnvVar {
                name: "S3_BUCKET".to_string(),
                value: Some(s3.bucket.clone()),
                ..Default::default()
            },
            EnvVar {
                name: "S3_PATH_PREFIX".to_string(),
                value: Some(path_prefix.to_string()),
                ..Default::default()
            },
            EnvVar {
                name: "AWS_REGION".to_string(),
                value: Some(s3.region.clone()),
                ..Default::default()
            },
            EnvVar {
                name: "AWS_DEFAULT_REGION".to_string(),
                value: Some(s3.region.clone()),
                ..Default::default()
            },
        ]),
        env_from: Some(vec![EnvFromSource {
            secret_ref: Some(SecretEnvSource {
                name: credentials_secret,
                optional: Some(false),
            }),
            ..Default::default()
        }]),
        volume_mounts: Some(vec![workdir_mount()]),
        ..Default::default()
    }
}

/// Dispatch on `output.type` to produce the appropriate `output-upload` container.
/// See `specs/004-pvc-backend/contracts/output-backends.md` for the contract.
///
/// The Oci arm needs `short_hash` and `scan_format` to construct the container's
/// `$(VAR)`-substituted args, so the dispatch fn takes the full spec rather
/// than just the `Output`.
fn build_output_upload_container(
    spec: &NamespaceScanSpec,
    short_hash: &str,
) -> Result<Container, BuildScanJobError> {
    let output = &spec.output;
    match &output.backend_type {
        OutputType::Pvc => {
            let pvc = output
                .pvc
                .as_ref()
                .ok_or(BuildScanJobError::MissingPvcConfig)?;
            if pvc.claim_name.trim().is_empty() {
                return Err(BuildScanJobError::MissingPvcConfig);
            }
            Ok(build_output_upload_pvc_container(pvc))
        }
        OutputType::S3 => {
            let s3 = output
                .s3
                .as_ref()
                .ok_or(BuildScanJobError::MissingS3Config)?;
            if s3.bucket.trim().is_empty()
                || s3
                    .credentials_secret_name
                    .as_deref()
                    .map(|s| s.trim().is_empty())
                    .unwrap_or(true)
            {
                return Err(BuildScanJobError::MissingS3Config);
            }
            Ok(build_output_upload_s3_container(s3))
        }
        OutputType::Oci => {
            let oci = output
                .oci
                .as_ref()
                .ok_or(BuildScanJobError::MissingOciConfig)?;
            if oci.registry.trim().is_empty()
                || oci.repository.trim().is_empty()
                || oci
                    .credentials_secret_name
                    .as_deref()
                    .map(|s| s.trim().is_empty())
                    .unwrap_or(true)
            {
                return Err(BuildScanJobError::MissingOciConfig);
            }
            Ok(build_output_upload_oci_container(
                oci,
                short_hash,
                &spec.scan_format,
            ))
        }
    }
}

/// Pod-spec `Volume` projected from a docker-registry `Secret` of type
/// `kubernetes.io/dockerconfigjson`. The Secret's standard `.dockerconfigjson`
/// key is remapped to `config.json` inside the volume so `DOCKER_CONFIG`-aware
/// tools (ORAS, crane, docker) find credentials at the expected path.
fn oci_credentials_volume(secret_name: &str) -> Volume {
    Volume {
        name: defaults::DOCKER_CONFIG_VOLUME_NAME.to_string(),
        secret: Some(SecretVolumeSource {
            secret_name: Some(secret_name.to_string()),
            items: Some(vec![KeyToPath {
                key: ".dockerconfigjson".to_string(),
                path: "config.json".to_string(),
                ..Default::default()
            }]),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Read-only mount of the docker-registry Secret inside `output-upload`.
fn oci_credentials_mount() -> VolumeMount {
    VolumeMount {
        name: defaults::DOCKER_CONFIG_VOLUME_NAME.to_string(),
        mount_path: defaults::DOCKER_CONFIG_MOUNT_PATH.to_string(),
        read_only: Some(true),
        ..Default::default()
    }
}

/// Oci-variant `output-upload` container — `oras push` of the SBOM as an OCI
/// artifact tagged with the image hash. ORAS is distroless (no shell), so all
/// variable expansion uses k8s container-arg `$(VAR)` substitution. The SBOM
/// file path (`/workdir/out/<hash>.<ext>`) is fully known at builder time;
/// `IMAGE_HASH` and `SBOM_EXT` env vars are populated for the substitution.
fn build_output_upload_oci_container(
    oci: &OciOutput,
    short_hash: &str,
    scan_format: &ScanFormat,
) -> Container {
    let (_format_arg, sbom_ext) = scan_format_args(scan_format);
    Container {
        name: "output-upload".to_string(),
        image: Some(defaults::OCI_UPLOAD_IMAGE.to_string()),
        // ORAS has ENTRYPOINT=["oras"] so we pass push args directly.
        args: Some(vec![
            "push".to_string(),
            "$(OCI_REGISTRY)/$(OCI_REPOSITORY):$(IMAGE_HASH)".to_string(),
            "/workdir/out/$(IMAGE_HASH).$(SBOM_EXT):application/json".to_string(),
        ]),
        env: Some(vec![
            EnvVar {
                name: "OCI_REGISTRY".to_string(),
                value: Some(oci.registry.clone()),
                ..Default::default()
            },
            EnvVar {
                name: "OCI_REPOSITORY".to_string(),
                value: Some(oci.repository.clone()),
                ..Default::default()
            },
            EnvVar {
                name: "IMAGE_HASH".to_string(),
                value: Some(short_hash.to_string()),
                ..Default::default()
            },
            EnvVar {
                name: "SBOM_EXT".to_string(),
                value: Some(sbom_ext.to_string()),
                ..Default::default()
            },
            EnvVar {
                name: "DOCKER_CONFIG".to_string(),
                value: Some(defaults::DOCKER_CONFIG_MOUNT_PATH.to_string()),
                ..Default::default()
            },
        ]),
        volume_mounts: Some(vec![workdir_mount(), oci_credentials_mount()]),
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
            mikebom_image: "ghcr.io/kusari-oss/mikebom:v0.1.0-alpha.64".to_string(),
            scan_format: ScanFormat::CyclonedxJson,
            // Feature 006: all 3 backends are now real; there is no placeholder
            // branch left. Populate a valid OCI config so inherited tests that
            // assert generic Job-shape invariants continue to succeed. Tests
            // that care about *specific* dispatch shape use a per-backend
            // fixture (valid_pvc_spec, valid_s3_spec, valid_oci_spec) and
            // override the relevant fields.
            output: Output {
                backend_type: OutputType::Oci,
                pvc: None,
                s3: None,
                oci: Some(OciOutput {
                    registry: "ghcr.io".to_string(),
                    repository: "kusari-oss/sboms".to_string(),
                    credentials_secret_name: Some("registry-creds".to_string()),
                }),
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
            Some("ghcr.io/kusari-oss/mikebom:v0.1.0-alpha.64"),
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

    // (Feature 003's `output_upload_oci_is_v03_placeholder` was deleted in
    // feature 006: there is no more placeholder dispatch arm. All 3 backends
    // now produce real upload containers. OCI's real shape is covered by the
    // feature-006 tests below.)

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

    // ---------- Feature 004 — PVC output backend (US1) ----------

    use crate::crds::namespace_scan::PvcOutput;

    fn valid_pvc_spec(claim_name: &str, path_prefix: Option<&str>) -> NamespaceScanSpec {
        let mut spec = valid_spec();
        spec.output = Output {
            backend_type: OutputType::Pvc,
            pvc: Some(PvcOutput {
                claim_name: claim_name.to_string(),
                path_prefix: path_prefix.map(|s| s.to_string()),
            }),
            s3: None,
            oci: None,
        };
        spec
    }

    #[test]
    fn pvc_dispatch_adds_pvc_volume_to_pod_spec() {
        let job = build_scan_job(
            &valid_pvc_spec("sbom-scratch", None),
            "scan-prod",
            "nginx:1.27.0",
        )
        .unwrap();
        let ps = pod_spec(&job);
        let volumes = ps.volumes.as_ref().unwrap();
        let pvc_vol = volumes
            .iter()
            .find(|v| v.name == defaults::PVC_VOLUME_NAME)
            .expect("pvc-output volume must be present");
        let pvc_src = pvc_vol
            .persistent_volume_claim
            .as_ref()
            .expect("pvc volume needs persistent_volume_claim source");
        assert_eq!(pvc_src.claim_name, "sbom-scratch");
    }

    #[test]
    fn pvc_output_upload_mounts_pvc_at_known_path() {
        let job = build_scan_job(
            &valid_pvc_spec("sbom-scratch", None),
            "scan-prod",
            "nginx:1.27.0",
        )
        .unwrap();
        let upload = &pod_spec(&job).containers[0];
        let mounts = upload.volume_mounts.as_ref().unwrap();
        assert!(
            mounts
                .iter()
                .any(|m| m.name == defaults::PVC_VOLUME_NAME
                    && m.mount_path == defaults::PVC_MOUNT_PATH),
            "output-upload missing {} mount at {}",
            defaults::PVC_VOLUME_NAME,
            defaults::PVC_MOUNT_PATH,
        );
    }

    #[test]
    fn pvc_volume_mounted_only_on_output_upload() {
        let job = build_scan_job(
            &valid_pvc_spec("sbom-scratch", None),
            "scan-prod",
            "nginx:1.27.0",
        )
        .unwrap();
        let ps = pod_spec(&job);
        // init-pull (index 0) and mikebom-scan (index 1) MUST NOT mount the PVC.
        for c in ps.init_containers.as_ref().unwrap() {
            let mounts = c.volume_mounts.as_ref().unwrap();
            assert!(
                !mounts.iter().any(|m| m.name == defaults::PVC_VOLUME_NAME),
                "container {} unexpectedly mounts {}",
                c.name,
                defaults::PVC_VOLUME_NAME,
            );
        }
    }

    #[test]
    fn pvc_output_upload_copies_to_pvc_mount() {
        let job = build_scan_job(
            &valid_pvc_spec("sbom-scratch", None),
            "scan-prod",
            "nginx:1.27.0",
        )
        .unwrap();
        let upload = &pod_spec(&job).containers[0];
        let cmd = upload.command.as_ref().unwrap().join("\n");
        assert!(
            cmd.contains("mkdir -p"),
            "output-upload missing mkdir -p: {cmd}"
        );
        assert!(
            cmd.contains("cp /workdir/out/*.json"),
            "output-upload missing cp command: {cmd}",
        );
        assert!(
            cmd.contains("PATH_PREFIX"),
            "output-upload should read PATH_PREFIX env: {cmd}",
        );
    }

    #[test]
    fn pvc_output_upload_respects_path_prefix() {
        let job = build_scan_job(
            &valid_pvc_spec("sbom-scratch", Some("team-a")),
            "scan-prod",
            "nginx:1.27.0",
        )
        .unwrap();
        let upload = &pod_spec(&job).containers[0];
        let env = upload
            .env
            .as_ref()
            .expect("env required when pathPrefix set");
        assert!(
            env.iter()
                .any(|e| e.name == "PATH_PREFIX" && e.value.as_deref() == Some("team-a")),
            "PATH_PREFIX env should equal team-a; got {env:?}",
        );
    }

    #[test]
    fn path_prefix_strips_leading_slash() {
        let job = build_scan_job(
            &valid_pvc_spec("sbom-scratch", Some("/team-a")),
            "scan-prod",
            "nginx:1.27.0",
        )
        .unwrap();
        let upload = &pod_spec(&job).containers[0];
        let env = upload.env.as_ref().unwrap();
        assert!(
            env.iter()
                .any(|e| e.name == "PATH_PREFIX" && e.value.as_deref() == Some("team-a")),
            "leading-slash strip should produce team-a; got {env:?}",
        );
    }

    #[test]
    fn missing_pvc_config_errors_when_pvc_none() {
        let mut spec = valid_spec();
        spec.output.backend_type = OutputType::Pvc;
        spec.output.pvc = None;
        let err = build_scan_job(&spec, "scan-prod", "nginx:1.27.0").unwrap_err();
        assert_eq!(err, BuildScanJobError::MissingPvcConfig);
    }

    #[test]
    fn missing_pvc_config_errors_when_claim_empty() {
        let err1 =
            build_scan_job(&valid_pvc_spec("", None), "scan-prod", "nginx:1.27.0").unwrap_err();
        assert_eq!(err1, BuildScanJobError::MissingPvcConfig);
        let err2 =
            build_scan_job(&valid_pvc_spec("   ", None), "scan-prod", "nginx:1.27.0").unwrap_err();
        assert_eq!(err2, BuildScanJobError::MissingPvcConfig);
    }

    // ---------- Feature 005 — S3 output backend ----------

    fn valid_s3_spec(
        bucket: &str,
        region: &str,
        credentials_secret_name: Option<&str>,
        path_prefix: Option<&str>,
    ) -> NamespaceScanSpec {
        let mut spec = valid_spec();
        spec.output = Output {
            backend_type: OutputType::S3,
            pvc: None,
            s3: Some(S3Output {
                bucket: bucket.to_string(),
                region: region.to_string(),
                path_prefix: path_prefix.map(|s| s.to_string()),
                credentials_secret_name: credentials_secret_name.map(|s| s.to_string()),
            }),
            oci: None,
        };
        spec
    }

    #[test]
    fn s3_dispatch_uses_aws_cli_image() {
        let spec = valid_s3_spec("sboms-prod", "us-west-2", Some("aws-creds"), None);
        let job = build_scan_job(&spec, "scan-prod", "nginx:1.27.0").unwrap();
        let upload = &pod_spec(&job).containers[0];
        assert_eq!(upload.image.as_deref(), Some(defaults::S3_UPLOAD_IMAGE));
    }

    #[test]
    fn s3_output_upload_uses_aws_s3_cp() {
        let spec = valid_s3_spec("sboms-prod", "us-west-2", Some("aws-creds"), None);
        let job = build_scan_job(&spec, "scan-prod", "nginx:1.27.0").unwrap();
        let upload = &pod_spec(&job).containers[0];
        let cmd = upload.command.as_ref().unwrap().join("\n");
        assert!(
            cmd.contains("aws s3 cp /workdir/out/"),
            "missing aws s3 cp: {cmd}",
        );
        assert!(cmd.contains("S3_BUCKET"), "missing S3_BUCKET ref: {cmd}");
        assert!(
            cmd.contains("--include '*.json'"),
            "missing json-only filter: {cmd}",
        );
    }

    #[test]
    fn s3_env_carries_bucket_and_region() {
        let spec = valid_s3_spec("sboms-prod", "us-west-2", Some("aws-creds"), None);
        let job = build_scan_job(&spec, "scan-prod", "nginx:1.27.0").unwrap();
        let upload = &pod_spec(&job).containers[0];
        let env = upload.env.as_ref().unwrap();
        assert!(env
            .iter()
            .any(|e| e.name == "S3_BUCKET" && e.value.as_deref() == Some("sboms-prod")));
        assert!(env
            .iter()
            .any(|e| e.name == "AWS_REGION" && e.value.as_deref() == Some("us-west-2")));
        assert!(env
            .iter()
            .any(|e| e.name == "AWS_DEFAULT_REGION" && e.value.as_deref() == Some("us-west-2")));
    }

    #[test]
    fn s3_credentials_come_from_secret_via_envfrom() {
        let spec = valid_s3_spec("sboms-prod", "us-west-2", Some("aws-creds"), None);
        let job = build_scan_job(&spec, "scan-prod", "nginx:1.27.0").unwrap();
        let upload = &pod_spec(&job).containers[0];
        let env_from = upload
            .env_from
            .as_ref()
            .expect("env_from must be set for S3");
        assert!(
            env_from.iter().any(|e| e
                .secret_ref
                .as_ref()
                .is_some_and(|sr| sr.name == "aws-creds")),
            "envFrom should reference secret 'aws-creds'; got {env_from:?}",
        );
    }

    #[test]
    fn s3_output_upload_respects_path_prefix() {
        let spec = valid_s3_spec("sboms-prod", "us-west-2", Some("aws-creds"), Some("team-a"));
        let job = build_scan_job(&spec, "scan-prod", "nginx:1.27.0").unwrap();
        let upload = &pod_spec(&job).containers[0];
        let env = upload.env.as_ref().unwrap();
        assert!(env
            .iter()
            .any(|e| e.name == "S3_PATH_PREFIX" && e.value.as_deref() == Some("team-a")));
    }

    #[test]
    fn s3_path_prefix_strips_leading_slash() {
        let spec = valid_s3_spec(
            "sboms-prod",
            "us-west-2",
            Some("aws-creds"),
            Some("/team-a"),
        );
        let job = build_scan_job(&spec, "scan-prod", "nginx:1.27.0").unwrap();
        let upload = &pod_spec(&job).containers[0];
        let env = upload.env.as_ref().unwrap();
        assert!(env
            .iter()
            .any(|e| e.name == "S3_PATH_PREFIX" && e.value.as_deref() == Some("team-a")));
    }

    #[test]
    fn missing_s3_config_errors_when_s3_none() {
        let mut spec = valid_spec();
        spec.output.backend_type = OutputType::S3;
        spec.output.s3 = None;
        let err = build_scan_job(&spec, "scan-prod", "nginx:1.27.0").unwrap_err();
        assert_eq!(err, BuildScanJobError::MissingS3Config);
    }

    #[test]
    fn missing_s3_config_errors_when_bucket_or_secret_empty() {
        // Empty bucket → error
        let s = valid_s3_spec("", "us-west-2", Some("aws-creds"), None);
        assert_eq!(
            build_scan_job(&s, "scan-prod", "nginx:1.27.0").unwrap_err(),
            BuildScanJobError::MissingS3Config,
        );
        // Missing credentials secret → error
        let s = valid_s3_spec("sboms-prod", "us-west-2", None, None);
        assert_eq!(
            build_scan_job(&s, "scan-prod", "nginx:1.27.0").unwrap_err(),
            BuildScanJobError::MissingS3Config,
        );
        // Whitespace credentials secret → error
        let s = valid_s3_spec("sboms-prod", "us-west-2", Some("   "), None);
        assert_eq!(
            build_scan_job(&s, "scan-prod", "nginx:1.27.0").unwrap_err(),
            BuildScanJobError::MissingS3Config,
        );
    }

    // ---------- Feature 006 — OCI registry output backend ----------

    fn valid_oci_spec(
        registry: &str,
        repository: &str,
        credentials_secret_name: Option<&str>,
    ) -> NamespaceScanSpec {
        let mut spec = valid_spec();
        spec.output = Output {
            backend_type: OutputType::Oci,
            pvc: None,
            s3: None,
            oci: Some(OciOutput {
                registry: registry.to_string(),
                repository: repository.to_string(),
                credentials_secret_name: credentials_secret_name.map(|s| s.to_string()),
            }),
        };
        spec
    }

    #[test]
    fn oci_dispatch_uses_oras_image() {
        let spec = valid_oci_spec("ghcr.io", "kusari-oss/sboms", Some("registry-creds"));
        let job = build_scan_job(&spec, "scan-prod", "nginx:1.27.0").unwrap();
        let upload = &pod_spec(&job).containers[0];
        assert_eq!(upload.image.as_deref(), Some(defaults::OCI_UPLOAD_IMAGE));
    }

    #[test]
    fn oci_output_upload_uses_oras_push() {
        let spec = valid_oci_spec("ghcr.io", "kusari-oss/sboms", Some("registry-creds"));
        let job = build_scan_job(&spec, "scan-prod", "nginx:1.27.0").unwrap();
        let upload = &pod_spec(&job).containers[0];
        // ORAS is distroless — we pass args, not a command.
        let args = upload.args.as_ref().expect("oras args required");
        assert_eq!(args[0], "push");
        assert!(
            args[1].contains("$(OCI_REGISTRY)") && args[1].contains("$(OCI_REPOSITORY)"),
            "oras push target missing var substitution: {args:?}",
        );
        assert!(
            args[2].contains("/workdir/out/") && args[2].contains(":application/json"),
            "oras push file path missing /workdir/out/ or media type: {args:?}",
        );
    }

    #[test]
    fn oci_env_carries_registry_repository_and_image_hash() {
        let spec = valid_oci_spec("ghcr.io", "kusari-oss/sboms", Some("registry-creds"));
        let job = build_scan_job(&spec, "scan-prod", "nginx:1.27.0").unwrap();
        let upload = &pod_spec(&job).containers[0];
        let env = upload.env.as_ref().unwrap();
        assert!(env
            .iter()
            .any(|e| e.name == "OCI_REGISTRY" && e.value.as_deref() == Some("ghcr.io")));
        assert!(env
            .iter()
            .any(|e| e.name == "OCI_REPOSITORY" && e.value.as_deref() == Some("kusari-oss/sboms")));
        assert!(env.iter().any(|e| e.name == "IMAGE_HASH"
            && e.value
                .as_deref()
                .map(|v| !v.is_empty() && v.len() == 7)
                .unwrap_or(false)));
        assert!(env.iter().any(|e| e.name == "DOCKER_CONFIG"
            && e.value.as_deref() == Some(defaults::DOCKER_CONFIG_MOUNT_PATH)));
    }

    #[test]
    fn oci_credentials_volume_mounted_at_known_path() {
        let spec = valid_oci_spec("ghcr.io", "kusari-oss/sboms", Some("registry-creds"));
        let job = build_scan_job(&spec, "scan-prod", "nginx:1.27.0").unwrap();
        let ps = pod_spec(&job);
        let volumes = ps.volumes.as_ref().unwrap();
        let cred_vol = volumes
            .iter()
            .find(|v| v.name == defaults::DOCKER_CONFIG_VOLUME_NAME)
            .expect("oci-credentials volume must exist");
        let secret_src = cred_vol
            .secret
            .as_ref()
            .expect("oci-credentials must use a Secret projection");
        assert_eq!(secret_src.secret_name.as_deref(), Some("registry-creds"));
        // The .dockerconfigjson key must be remapped to config.json for
        // DOCKER_CONFIG-aware tools.
        let items = secret_src.items.as_ref().expect("items remap required");
        assert!(items
            .iter()
            .any(|i| i.key == ".dockerconfigjson" && i.path == "config.json"));

        // And output-upload must mount it at the conventional path, read-only.
        let upload = &ps.containers[0];
        let mounts = upload.volume_mounts.as_ref().unwrap();
        let cred_mount = mounts
            .iter()
            .find(|m| m.name == defaults::DOCKER_CONFIG_VOLUME_NAME)
            .expect("output-upload must mount the credentials volume");
        assert_eq!(cred_mount.mount_path, defaults::DOCKER_CONFIG_MOUNT_PATH);
        assert_eq!(cred_mount.read_only, Some(true));
    }

    #[test]
    fn oci_credentials_volume_only_on_output_upload() {
        let spec = valid_oci_spec("ghcr.io", "kusari-oss/sboms", Some("registry-creds"));
        let job = build_scan_job(&spec, "scan-prod", "nginx:1.27.0").unwrap();
        let ps = pod_spec(&job);
        // FR-007 blast-radius: init-pull and mikebom-scan must NOT mount the
        // registry credentials — a compromised scan can't exfiltrate them.
        for c in ps.init_containers.as_ref().unwrap() {
            let mounts = c.volume_mounts.as_ref().unwrap();
            assert!(
                !mounts
                    .iter()
                    .any(|m| m.name == defaults::DOCKER_CONFIG_VOLUME_NAME),
                "container {} unexpectedly mounts oci credentials",
                c.name,
            );
        }
    }

    #[test]
    fn oci_sbom_ext_matches_scan_format() {
        for (format, expected_ext) in [
            (ScanFormat::CyclonedxJson, "cdx.json"),
            (ScanFormat::Spdx23Json, "spdx.json"),
            (ScanFormat::Spdx3Json, "spdx3.json"),
        ] {
            let mut spec = valid_oci_spec("ghcr.io", "kusari-oss/sboms", Some("registry-creds"));
            spec.scan_format = format;
            let job = build_scan_job(&spec, "scan-prod", "nginx:1.27.0").unwrap();
            let upload = &pod_spec(&job).containers[0];
            let env = upload.env.as_ref().unwrap();
            assert!(
                env.iter()
                    .any(|e| e.name == "SBOM_EXT" && e.value.as_deref() == Some(expected_ext)),
                "SBOM_EXT should be {expected_ext}; got env {env:?}",
            );
        }
    }

    #[test]
    fn missing_oci_config_errors_when_oci_none() {
        let mut spec = valid_spec();
        spec.output.backend_type = OutputType::Oci;
        spec.output.oci = None;
        let err = build_scan_job(&spec, "scan-prod", "nginx:1.27.0").unwrap_err();
        assert_eq!(err, BuildScanJobError::MissingOciConfig);
    }

    #[test]
    fn missing_oci_config_errors_when_required_fields_empty() {
        // Empty registry
        let s = valid_oci_spec("", "kusari-oss/sboms", Some("registry-creds"));
        assert_eq!(
            build_scan_job(&s, "scan-prod", "nginx:1.27.0").unwrap_err(),
            BuildScanJobError::MissingOciConfig,
        );
        // Empty repository
        let s = valid_oci_spec("ghcr.io", "", Some("registry-creds"));
        assert_eq!(
            build_scan_job(&s, "scan-prod", "nginx:1.27.0").unwrap_err(),
            BuildScanJobError::MissingOciConfig,
        );
        // Missing credentials
        let s = valid_oci_spec("ghcr.io", "kusari-oss/sboms", None);
        assert_eq!(
            build_scan_job(&s, "scan-prod", "nginx:1.27.0").unwrap_err(),
            BuildScanJobError::MissingOciConfig,
        );
        // Whitespace credentials
        let s = valid_oci_spec("ghcr.io", "kusari-oss/sboms", Some("   "));
        assert_eq!(
            build_scan_job(&s, "scan-prod", "nginx:1.27.0").unwrap_err(),
            BuildScanJobError::MissingOciConfig,
        );
    }
}
