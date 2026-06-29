//! Reads owned scan-Job statuses, aggregates them into a single
//! `AggregatedOutcome`, and feeds it to `crate::status::status_with_aggregated_outcome`.
//!
//! Public surface: `ensure_jobs`'s peer — `list_owned_jobs` (I/O) and
//! `aggregate_job_outcomes` (pure). All sub-helpers are `pub(crate)` for
//! testability.
//!
//! See:
//! - `specs/008-job-status-feedback/contracts/status-aggregator.md`
//! - `specs/008-job-status-feedback/data-model.md`
//! - `specs/008-job-status-feedback/research.md`

use k8s_openapi::api::batch::v1::Job;
use kube::{api::ListParams, Api};

use crate::crds::namespace_scan::{NamespaceScanSpec, OutputType, ScannedImage};

/// Result of a single `aggregate_job_outcomes` invocation. Drives the
/// `status_with_aggregated_outcome` decision table (research §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AggregatedOutcome {
    /// Every owned Job has `status.succeeded >= 1`. `scanned` carries one
    /// `ScannedImage` per Job — see research.md §6 for the construction order.
    AllSucceeded { scanned: Vec<ScannedImage> },

    /// At least one owned Job has `status.failed >= backoffLimit + 1`.
    /// `image_ref` carries the first failing image (deterministic by iteration
    /// order over the input slice). Failure dominates partial-success.
    AnyFailed { image_ref: String },

    /// Default: empty Job list, or some Jobs still running/retrying within
    /// budget. Preserves whatever reason `base` already has (Scanning from
    /// feature 007's mapper).
    StillRunning,
}

// =============================================================================
// Pure helpers (no I/O — fully unit-testable).
// =============================================================================

/// `true` iff the Job's `.status.succeeded >= 1` (k8s guarantees succeeded is
/// incremented to 1 only on terminal success).
pub(crate) fn is_job_succeeded(job: &Job) -> bool {
    job.status.as_ref().and_then(|s| s.succeeded).unwrap_or(0) >= 1
}

/// `true` iff the Job is finally failed per k8s semantics —
/// `.status.failed >= .spec.backoffLimit + 1`. The `+ 1` comes from k8s's
/// documented rule: a Job is "finally failed" only after the
/// `(backoffLimit + 1)`th pod fails.
pub(crate) fn is_job_finally_failed(job: &Job) -> bool {
    let failed = job.status.as_ref().and_then(|s| s.failed).unwrap_or(0);
    let backoff_limit = job.spec.as_ref().and_then(|s| s.backoff_limit).unwrap_or(6);
    failed > backoff_limit
}

/// Read `IMAGE_REF` from the Job's `init-pull` container's env. Feature 003's
/// builder sets this env var; missing init-pull container or missing env var
/// → `None` (that Job contributes nothing to `scanned` but still counts for
/// aggregation).
pub(crate) fn extract_image_ref_from_job(job: &Job) -> Option<String> {
    let init_containers = job
        .spec
        .as_ref()?
        .template
        .spec
        .as_ref()?
        .init_containers
        .as_ref()?;
    let init_pull = init_containers.iter().find(|c| c.name == "init-pull")?;
    let env = init_pull.env.as_ref()?;
    let entry = env.iter().find(|e| e.name == "IMAGE_REF")?;
    entry.value.clone()
}

/// Derive the user-facing SBOM artifact URL from the CR's `output` block +
/// the Job's short-image-hash (label `kusari.dev/image-ref-hash`). See FR-008.
/// Returns `<backend>://unknown` for the defensive case where the matching
/// `spec.output.{pvc,s3,oci}` block is `None` — this should never happen in
/// practice (the orchestrator's `BuildFailed` arm would have caught it).
pub(crate) fn derive_sbom_location(spec: &NamespaceScanSpec, short_hash: &str) -> String {
    match spec.output.backend_type {
        OutputType::Pvc => {
            let Some(pvc) = &spec.output.pvc else {
                return "pvc://unknown".to_string();
            };
            let claim = pvc.claim_name.trim();
            let prefix = pvc.path_prefix.as_deref().unwrap_or("").trim();
            if prefix.is_empty() {
                format!("pvc://{claim}/{short_hash}.json")
            } else {
                format!("pvc://{claim}/{prefix}/{short_hash}.json")
            }
        }
        OutputType::S3 => {
            let Some(s3) = &spec.output.s3 else {
                return "s3://unknown".to_string();
            };
            let bucket = s3.bucket.trim();
            let prefix = s3.path_prefix.as_deref().unwrap_or("").trim();
            if prefix.is_empty() {
                format!("s3://{bucket}/{short_hash}.json")
            } else {
                format!("s3://{bucket}/{prefix}/{short_hash}.json")
            }
        }
        OutputType::Oci => {
            let Some(oci) = &spec.output.oci else {
                return "oci://unknown".to_string();
            };
            let registry = oci.registry.trim();
            let repository = oci.repository.trim();
            format!("oci://{registry}/{repository}:{short_hash}")
        }
    }
}

/// Merge `newly_completed` into `existing`, append-only per FR-015.
/// Duplicate `image_ref` keys favor the newly-completed entry (newest wins).
/// Output is sorted by `image_ref` (deterministic).
pub(crate) fn merge_scanned_images_append_only(
    existing: &[ScannedImage],
    newly_completed: Vec<ScannedImage>,
) -> Vec<ScannedImage> {
    use std::collections::BTreeMap;
    let mut by_ref: BTreeMap<String, ScannedImage> = existing
        .iter()
        .cloned()
        .map(|s| (s.image_ref.clone(), s))
        .collect();
    for s in newly_completed {
        by_ref.insert(s.image_ref.clone(), s);
    }
    by_ref.into_values().collect()
}

// =============================================================================
// Aggregator — pure function over a list of Jobs.
// =============================================================================

/// Top-level pure aggregator. See contracts/status-aggregator.md for invariants.
pub fn aggregate_job_outcomes(jobs: &[Job], spec: &NamespaceScanSpec) -> AggregatedOutcome {
    // Empty list → StillRunning (research §6; avoids ScanCompleted-flapping
    // in the narrow window where TTL fires between create and list).
    if jobs.is_empty() {
        return AggregatedOutcome::StillRunning;
    }

    // Failure dominates: any finally-failed Job → AnyFailed { first failing }.
    if let Some(failed) = jobs.iter().find(|j| is_job_finally_failed(j)) {
        let image_ref =
            extract_image_ref_from_job(failed).unwrap_or_else(|| "<unknown>".to_string());
        return AggregatedOutcome::AnyFailed { image_ref };
    }

    // All succeeded → AllSucceeded; build one ScannedImage per Job.
    if jobs.iter().all(is_job_succeeded) {
        let scanned = jobs
            .iter()
            .filter_map(|job| scanned_image_from_job(job, spec))
            .collect();
        return AggregatedOutcome::AllSucceeded { scanned };
    }

    // Otherwise: at least one Job is still in progress (retrying within
    // backoffLimit, or not yet succeeded).
    AggregatedOutcome::StillRunning
}

/// Build a `ScannedImage` from a succeeded Job. Skips Jobs without the
/// `IMAGE_REF` env var or the `kusari.dev/image-ref-hash` label (those would
/// produce a malformed entry).
fn scanned_image_from_job(job: &Job, spec: &NamespaceScanSpec) -> Option<ScannedImage> {
    let image_ref = extract_image_ref_from_job(job)?;
    let short_hash = job
        .metadata
        .labels
        .as_ref()?
        .get("kusari.dev/image-ref-hash")?
        .clone();
    let completed_at = job
        .status
        .as_ref()?
        .completion_time
        .as_ref()
        .map(|t| t.0.to_rfc3339())
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
    let sbom_location = derive_sbom_location(spec, &short_hash);
    Some(ScannedImage {
        image_ref,
        resolved_sha: None,
        sbom_location,
        completed_at,
    })
}

// =============================================================================
// I/O — list owned Jobs by label.
// =============================================================================

/// List every Job in the operator's namespace labeled with this CR's name
/// (`kusari.dev/namespace-scan=<cr_name>`). Used by the reconciler before
/// invoking `aggregate_job_outcomes`.
pub async fn list_owned_jobs(api: &Api<Job>, cr_name: &str) -> Result<Vec<Job>, kube::Error> {
    let selector = format!("kusari.dev/namespace-scan={cr_name}");
    api.list(&ListParams::default().labels(&selector))
        .await
        .map(|l| l.items)
}

// =============================================================================
// Tests.
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crds::namespace_scan::{
        OciOutput, Output, OutputType, PvcOutput, S3Output, ScanFormat, Schedule, Target,
    };
    use k8s_openapi::api::batch::v1::JobSpec;
    use k8s_openapi::api::batch::v1::JobStatus;
    use k8s_openapi::api::core::v1::{Container, EnvVar, PodSpec, PodTemplateSpec};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, Time};
    use std::collections::BTreeMap;

    // -----------------------------------------------------------------------
    // Fixture helpers
    // -----------------------------------------------------------------------

    fn build_job(
        cr_name: &str,
        short_hash: &str,
        image_ref: &str,
        succeeded: Option<i32>,
        failed: Option<i32>,
        backoff_limit: Option<i32>,
    ) -> Job {
        let labels: BTreeMap<String, String> = BTreeMap::from([
            ("kusari.dev/namespace-scan".to_string(), cr_name.to_string()),
            (
                "kusari.dev/image-ref-hash".to_string(),
                short_hash.to_string(),
            ),
        ]);
        Job {
            metadata: ObjectMeta {
                name: Some(format!("nsscan-{cr_name}-{short_hash}")),
                labels: Some(labels),
                ..Default::default()
            },
            spec: Some(JobSpec {
                backoff_limit,
                template: PodTemplateSpec {
                    spec: Some(PodSpec {
                        init_containers: Some(vec![Container {
                            name: "init-pull".to_string(),
                            env: Some(vec![EnvVar {
                                name: "IMAGE_REF".to_string(),
                                value: Some(image_ref.to_string()),
                                ..Default::default()
                            }]),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                ..Default::default()
            }),
            status: Some(JobStatus {
                succeeded,
                failed,
                completion_time: succeeded.map(|_| Time(chrono::Utc::now())),
                ..Default::default()
            }),
        }
    }

    fn s3_spec() -> NamespaceScanSpec {
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
            mikebom_image: "ghcr.io/kusari-oss/mikebom:test".to_string(),
            scan_format: ScanFormat::CyclonedxJson,
            output: Output {
                backend_type: OutputType::S3,
                pvc: None,
                s3: Some(S3Output {
                    bucket: "test-sboms".to_string(),
                    region: "us-west-2".to_string(),
                    path_prefix: Some("team-a".to_string()),
                    credentials_secret_name: Some("aws-creds".to_string()),
                }),
                oci: None,
            },
        }
    }

    fn pvc_spec(path_prefix: Option<&str>) -> NamespaceScanSpec {
        let mut spec = s3_spec();
        spec.output = Output {
            backend_type: OutputType::Pvc,
            pvc: Some(PvcOutput {
                claim_name: "sbom-claim".to_string(),
                path_prefix: path_prefix.map(|s| s.to_string()),
            }),
            s3: None,
            oci: None,
        };
        spec
    }

    fn oci_spec() -> NamespaceScanSpec {
        let mut spec = s3_spec();
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

    // -----------------------------------------------------------------------
    // T007 — empty list → StillRunning
    // -----------------------------------------------------------------------

    #[test]
    fn t007_aggregate_empty_list_is_still_running() {
        let outcome = aggregate_job_outcomes(&[], &s3_spec());
        assert_eq!(outcome, AggregatedOutcome::StillRunning);
    }

    // -----------------------------------------------------------------------
    // T008 — all-succeeded → AllSucceeded
    // -----------------------------------------------------------------------

    #[test]
    fn t008_aggregate_all_succeeded_is_all_succeeded() {
        let jobs = vec![
            build_job("scan", "abc1234", "nginx:1.27.0", Some(1), None, Some(6)),
            build_job("scan", "def5678", "redis:7.4.0", Some(1), None, Some(6)),
        ];
        let outcome = aggregate_job_outcomes(&jobs, &s3_spec());
        match outcome {
            AggregatedOutcome::AllSucceeded { scanned } => {
                assert_eq!(scanned.len(), 2);
                let refs: Vec<&str> = scanned.iter().map(|s| s.image_ref.as_str()).collect();
                assert!(refs.contains(&"nginx:1.27.0"));
                assert!(refs.contains(&"redis:7.4.0"));
                for s in &scanned {
                    assert!(!s.image_ref.is_empty());
                    assert!(!s.sbom_location.is_empty());
                    assert!(!s.completed_at.is_empty());
                }
            }
            other => panic!("expected AllSucceeded, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // T009 — partial progress → StillRunning
    // -----------------------------------------------------------------------

    #[test]
    fn t009_aggregate_partial_progress_is_still_running() {
        let jobs = vec![
            build_job("scan", "abc1234", "nginx:1.27.0", Some(1), None, Some(6)),
            build_job("scan", "def5678", "redis:7.4.0", None, None, Some(6)),
        ];
        let outcome = aggregate_job_outcomes(&jobs, &s3_spec());
        assert_eq!(outcome, AggregatedOutcome::StillRunning);
    }

    // -----------------------------------------------------------------------
    // T019 — any finally-failed → AnyFailed
    // -----------------------------------------------------------------------

    #[test]
    fn t019_aggregate_finally_failed_is_any_failed() {
        let jobs = vec![build_job(
            "scan",
            "abc1234",
            "nginx:1.27.0",
            None,
            Some(7),
            Some(6),
        )];
        let outcome = aggregate_job_outcomes(&jobs, &s3_spec());
        match outcome {
            AggregatedOutcome::AnyFailed { image_ref } => {
                assert_eq!(image_ref, "nginx:1.27.0");
            }
            other => panic!("expected AnyFailed, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // T020 — mixed succeeded+failed → AnyFailed (failure dominates)
    // -----------------------------------------------------------------------

    #[test]
    fn t020_aggregate_mixed_state_is_any_failed() {
        let jobs = vec![
            build_job("scan", "abc1234", "nginx:1.27.0", Some(1), None, Some(6)),
            build_job("scan", "def5678", "redis:7.4.0", None, Some(7), Some(6)),
        ];
        let outcome = aggregate_job_outcomes(&jobs, &s3_spec());
        assert_eq!(
            outcome,
            AggregatedOutcome::AnyFailed {
                image_ref: "redis:7.4.0".to_string()
            }
        );
    }

    // -----------------------------------------------------------------------
    // T021 — retry-in-progress → StillRunning
    // -----------------------------------------------------------------------

    #[test]
    fn t021_aggregate_retry_in_progress_is_still_running() {
        let jobs = vec![build_job(
            "scan",
            "abc1234",
            "nginx:1.27.0",
            None,
            Some(3), // 3 retries used, backoff_limit=6 → not yet finally failed
            Some(6),
        )];
        let outcome = aggregate_job_outcomes(&jobs, &s3_spec());
        assert_eq!(outcome, AggregatedOutcome::StillRunning);
    }

    // -----------------------------------------------------------------------
    // T027 — derive_sbom_location across all 3 backends + path-prefix variants
    // -----------------------------------------------------------------------

    #[test]
    fn t027_derive_pvc_with_path_prefix() {
        let url = derive_sbom_location(&pvc_spec(Some("team-a")), "abc1234");
        assert_eq!(url, "pvc://sbom-claim/team-a/abc1234.json");
    }

    #[test]
    fn t027_derive_pvc_without_path_prefix() {
        let url = derive_sbom_location(&pvc_spec(None), "abc1234");
        assert_eq!(url, "pvc://sbom-claim/abc1234.json");
    }

    #[test]
    fn t027_derive_pvc_empty_path_prefix() {
        let url = derive_sbom_location(&pvc_spec(Some("")), "abc1234");
        assert_eq!(url, "pvc://sbom-claim/abc1234.json");
    }

    #[test]
    fn t027_derive_s3_with_path_prefix() {
        let url = derive_sbom_location(&s3_spec(), "abc1234");
        assert_eq!(url, "s3://test-sboms/team-a/abc1234.json");
    }

    #[test]
    fn t027_derive_s3_without_path_prefix() {
        let mut spec = s3_spec();
        spec.output.s3.as_mut().unwrap().path_prefix = None;
        let url = derive_sbom_location(&spec, "abc1234");
        assert_eq!(url, "s3://test-sboms/abc1234.json");
    }

    #[test]
    fn t027_derive_oci() {
        let url = derive_sbom_location(&oci_spec(), "abc1234");
        assert_eq!(url, "oci://ghcr.io/kusari-oss/sboms:abc1234");
    }

    // -----------------------------------------------------------------------
    // T028 — merge_scanned_images_append_only never removes; newest wins
    // -----------------------------------------------------------------------

    fn sample(image_ref: &str, completed_at: &str, sbom_location: &str) -> ScannedImage {
        ScannedImage {
            image_ref: image_ref.to_string(),
            resolved_sha: None,
            sbom_location: sbom_location.to_string(),
            completed_at: completed_at.to_string(),
        }
    }

    #[test]
    fn t028_merge_append_only_new_entries_appended() {
        let existing = vec![sample("nginx:1", "2026-06-28T10:00:00Z", "pvc://c/n.json")];
        let new_only = vec![sample("redis:7", "2026-06-28T10:05:00Z", "pvc://c/r.json")];
        let out = merge_scanned_images_append_only(&existing, new_only);
        assert_eq!(out.len(), 2);
        assert!(out.iter().any(|s| s.image_ref == "nginx:1"));
        assert!(out.iter().any(|s| s.image_ref == "redis:7"));
    }

    #[test]
    fn t028_merge_append_only_existing_preserved_when_new_is_empty() {
        let existing = vec![
            sample("nginx:1", "2026-06-28T10:00:00Z", "pvc://c/n.json"),
            sample("redis:7", "2026-06-28T10:01:00Z", "pvc://c/r.json"),
        ];
        let out = merge_scanned_images_append_only(&existing, vec![]);
        assert_eq!(
            out.len(),
            2,
            "existing entries MUST NOT be removed (FR-015)"
        );
    }

    #[test]
    fn t028_merge_newest_wins_on_duplicate_image_ref() {
        let existing = vec![sample(
            "nginx:1",
            "2026-06-28T10:00:00Z",
            "pvc://c/n-old.json",
        )];
        let new_with_dup = vec![sample(
            "nginx:1",
            "2026-06-28T10:05:00Z",
            "pvc://c/n-new.json",
        )];
        let out = merge_scanned_images_append_only(&existing, new_with_dup);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].completed_at, "2026-06-28T10:05:00Z");
        assert_eq!(out[0].sbom_location, "pvc://c/n-new.json");
    }

    #[test]
    fn t028_merge_output_sorted_by_image_ref() {
        let existing = vec![sample("zeta:1", "t1", "p1")];
        let new = vec![sample("alpha:1", "t2", "p2"), sample("mu:1", "t3", "p3")];
        let out = merge_scanned_images_append_only(&existing, new);
        let order: Vec<&str> = out.iter().map(|s| s.image_ref.as_str()).collect();
        assert_eq!(order, vec!["alpha:1", "mu:1", "zeta:1"]);
    }

    // -----------------------------------------------------------------------
    // extract_image_ref_from_job — boundary cases
    // -----------------------------------------------------------------------

    #[test]
    fn extract_image_ref_present_returns_value() {
        let job = build_job("scan", "abc1234", "nginx:1.27.0", Some(1), None, Some(6));
        assert_eq!(
            extract_image_ref_from_job(&job).as_deref(),
            Some("nginx:1.27.0")
        );
    }

    #[test]
    fn extract_image_ref_missing_init_pull_returns_none() {
        let mut job = build_job("scan", "abc1234", "nginx:1.27.0", Some(1), None, Some(6));
        // Remove init containers entirely.
        job.spec
            .as_mut()
            .unwrap()
            .template
            .spec
            .as_mut()
            .unwrap()
            .init_containers = None;
        assert_eq!(extract_image_ref_from_job(&job), None);
    }
}
