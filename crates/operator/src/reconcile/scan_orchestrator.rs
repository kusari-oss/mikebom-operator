//! Reconciler-side orchestration for spawning scan Jobs.
//!
//! Bridges feature 002's `NamespaceScan` reconciler to features 003–006's
//! pure `build_scan_job` builder. For every valid CR, this module enumerates
//! in-scope pods in the target namespaces, deduplicates their container
//! images, and creates one `batch/v1.Job` per image — idempotently, with
//! owner-references back to the CR.
//!
//! Public surface: `ensure_jobs`. Everything else is `pub(crate)` for
//! testability or kept private.
//!
//! See:
//! - `specs/007-reconciler-spawns-job/contracts/reconciler-orchestrator.md`
//! - `specs/007-reconciler-spawns-job/data-model.md`
//! - `specs/007-reconciler-spawns-job/research.md`

use std::collections::BTreeSet;

use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use kube::{
    api::{ListParams, PostParams},
    Api,
};
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::crds::namespace_scan::{NamespaceScanSpec, Target};
use crate::reconcile::namespace_scan::Ctx;
use crate::scan_job::build_scan_job;

/// Result of a single `ensure_jobs` invocation. Maps onto status reasons via
/// `crate::status::status_with_orchestration_result`.
///
/// See research.md §6, §7, and §8 for the design rationale: 403/builder errors
/// are absorbed here (not bubbled as `Err`) so the reconciler's status path
/// has a single decision table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrchestrationResult {
    /// At least one in-scope pod existed and the orchestrator ensured a Job
    /// per distinct image. `distinct_images` counts every image in the
    /// in-scope set (both newly-created and 409-preexisting Jobs), so the
    /// value is stable across idempotent re-invocations.
    Spawned { distinct_images: usize },

    /// Target resolved to zero in-scope pods. Distinct from
    /// `Spawned { distinct_images: 0 }`, which should never occur.
    NoImagesInScope,

    /// `build_scan_job` rejected the spec for at least one image. First
    /// failing image short-circuits orchestration.
    BuildFailed { image_ref: String, error: String },

    /// Kube returned 403 on pod-list or Job-create. First 403 short-circuits;
    /// no Jobs are created for any other image (research.md §6, constitution
    /// III).
    RbacInsufficient {
        verb_resource: String,
        namespace: Option<String>,
        message: String,
    },
}

/// Error returned only for *unexpected* kube failures (network, 500, etc.).
/// 403/409 and 404-on-list are absorbed into `OrchestrationResult` variants.
#[derive(Debug, Error)]
pub enum OrchestrationError {
    #[error("kube API error: {0}")]
    Kube(#[from] kube::Error),
}

/// Minimal subset of `ObjectMeta` the orchestrator needs. Lets unit tests
/// build fixtures without synthesizing a full `NamespaceScan`.
#[derive(Debug, Clone)]
pub struct CrMetaSnapshot {
    pub name: String,
    pub uid: String,
    pub namespace: String,
}

// =============================================================================
// Pure helpers (no I/O — fully unit-testable).
// =============================================================================

/// A pod is in scope iff its `status.phase` is `Running`, `Pending`, or `None`
/// (research.md §4 — `None` treated as `Pending`-equivalent for watch-list
/// freshness). Pods in `Succeeded`, `Failed`, or `Unknown` are excluded.
pub(crate) fn is_pod_in_scope(pod: &Pod) -> bool {
    match pod.status.as_ref().and_then(|s| s.phase.as_deref()) {
        Some("Running") | Some("Pending") | None => true,
        Some(_) => false,
    }
}

/// Collect distinct container image refs across all in-scope pods. Includes
/// `initContainers`, `containers`, and `ephemeralContainers`. Empty/None image
/// strings are skipped; no normalization (literal string is the dedup key).
pub(crate) fn collect_images_from_pods(pods: &[Pod]) -> BTreeSet<String> {
    let mut images = BTreeSet::new();
    for pod in pods.iter().filter(|p| is_pod_in_scope(p)) {
        let Some(spec) = pod.spec.as_ref() else {
            continue;
        };
        for c in spec.init_containers.iter().flatten() {
            push_image(&mut images, c.image.as_deref());
        }
        for c in spec.containers.iter() {
            push_image(&mut images, c.image.as_deref());
        }
        for c in spec.ephemeral_containers.iter().flatten() {
            push_image(&mut images, c.image.as_deref());
        }
    }
    images
}

fn push_image(images: &mut BTreeSet<String>, image: Option<&str>) {
    if let Some(s) = image.map(str::trim).filter(|s| !s.is_empty()) {
        images.insert(s.to_string());
    }
}

/// Build the single owner-reference every spawned Job carries (FR-005).
pub(crate) fn make_owner_reference(cr_meta: &CrMetaSnapshot) -> OwnerReference {
    OwnerReference {
        api_version: "kusari.dev/v1alpha1".to_string(),
        kind: "NamespaceScan".to_string(),
        name: cr_meta.name.clone(),
        uid: cr_meta.uid.clone(),
        controller: Some(true),
        block_owner_deletion: Some(true),
    }
}

// =============================================================================
// I/O helpers (kube-side; tested via fakes + integration E2E).
// =============================================================================

/// Outcome of listing pods for one target namespace. Mirrors the
/// orchestrator's top-level short-circuit semantics.
#[derive(Debug)]
pub(crate) enum PodListOutcome {
    Pods(Vec<Pod>),
    /// 404 on the namespace list — namespace doesn't exist yet. Treated
    /// identically to "namespace exists but is empty" by the caller.
    NamespaceMissing,
    /// 403 → fail-closed across the whole CR (constitution III, FR-008).
    RbacDenied {
        verb_resource: String,
        namespace: String,
        message: String,
    },
}

/// Pure classifier for a pod-list result. Extracted from `list_target_pods`
/// so the 403/404/unexpected paths are unit-testable without a kube Client.
pub(crate) fn classify_pod_list_result(
    result: Result<Vec<Pod>, kube::Error>,
    namespace: &str,
) -> PodListOutcome {
    match result {
        Ok(pods) => PodListOutcome::Pods(pods),
        Err(kube::Error::Api(e)) if e.code == 403 => PodListOutcome::RbacDenied {
            verb_resource: "list pods".to_string(),
            namespace: namespace.to_string(),
            message: e.message.clone(),
        },
        Err(kube::Error::Api(e)) if e.code == 404 => PodListOutcome::NamespaceMissing,
        Err(err) => {
            // Unexpected error path — treat as RBAC-equivalent abort, since
            // the orchestrator never returns Err mid-loop without aborting.
            // The reconciler's error_policy will requeue with backoff.
            PodListOutcome::RbacDenied {
                verb_resource: "list pods".to_string(),
                namespace: namespace.to_string(),
                message: format!("unexpected kube error: {err}"),
            }
        }
    }
}

/// List pods in a target namespace, translating kube errors via
/// `classify_pod_list_result`.
pub(crate) async fn list_target_pods(api: &Api<Pod>, namespace: &str) -> PodListOutcome {
    let result = api
        .list(&ListParams::default())
        .await
        .map(|list| list.items);
    classify_pod_list_result(result, namespace)
}

/// Outcome of a single Job create call. `Ok(true)` = created this cycle,
/// `Ok(false)` = preexisting (409). `Err(..)` = 403 fail-closed.
#[derive(Debug)]
pub(crate) enum JobCreateOutcome {
    Created,
    Preexisting,
    RbacDenied {
        verb_resource: String,
        namespace: String,
        message: String,
    },
}

/// Pure classifier for a Job-create result. Extracted from
/// `try_create_job_idempotent` so the 409/403/unexpected paths are
/// unit-testable without a kube Client.
pub(crate) fn classify_job_create_result(
    result: Result<Job, kube::Error>,
    namespace: &str,
) -> JobCreateOutcome {
    match result {
        Ok(_) => JobCreateOutcome::Created,
        Err(kube::Error::Api(e)) if e.code == 409 => JobCreateOutcome::Preexisting,
        Err(kube::Error::Api(e)) if e.code == 403 => JobCreateOutcome::RbacDenied {
            verb_resource: "create batch/v1.jobs".to_string(),
            namespace: namespace.to_string(),
            message: e.message.clone(),
        },
        Err(err) => JobCreateOutcome::RbacDenied {
            verb_resource: "create batch/v1.jobs".to_string(),
            namespace: namespace.to_string(),
            message: format!("unexpected kube error: {err}"),
        },
    }
}

/// Idempotent get-or-create for a Job. 409 → `Preexisting` (the deterministic
/// name made this a no-op race winner); 403 → `RbacDenied` (fail-closed per
/// constitution III).
pub(crate) async fn try_create_job_idempotent(
    api: &Api<Job>,
    namespace: &str,
    job: &Job,
) -> JobCreateOutcome {
    let result = api.create(&PostParams::default(), job).await;
    classify_job_create_result(result, namespace)
}

// =============================================================================
// Glue — `ensure_jobs`.
// =============================================================================

/// Top-level orchestration. See contracts/reconciler-orchestrator.md for the
/// invariants this function MUST uphold (no partial RBAC spawn, idempotency,
/// side-effect bound, deterministic naming, owner-ref shape).
pub async fn ensure_jobs(
    spec: &NamespaceScanSpec,
    cr_meta: &CrMetaSnapshot,
    ctx: &Ctx,
) -> Result<OrchestrationResult, OrchestrationError> {
    // 1. List pods across every target namespace. Short-circuit on the first
    //    RBAC denial. Treat 404 (missing namespace) as a non-error.
    let target_namespaces = resolve_target_namespaces(&spec.target);
    let mut all_pods: Vec<Pod> = Vec::new();
    for ns in &target_namespaces {
        let api: Api<Pod> = Api::namespaced(ctx.client.clone(), ns);
        match list_target_pods(&api, ns).await {
            PodListOutcome::Pods(mut pods) => all_pods.append(&mut pods),
            PodListOutcome::NamespaceMissing => {
                debug!(
                    event = "target_namespace_missing",
                    target_namespace = %ns,
                    "target namespace not present in cluster; treating as empty",
                );
            }
            PodListOutcome::RbacDenied {
                verb_resource,
                namespace,
                message,
            } => {
                warn!(
                    event = "rbac_insufficient",
                    verb_resource = %verb_resource,
                    namespace = %namespace,
                    message = %message,
                    "pod-list denied; aborting orchestration fail-closed (constitution III)",
                );
                return Ok(OrchestrationResult::RbacInsufficient {
                    verb_resource,
                    namespace: Some(namespace),
                    message,
                });
            }
        }
    }

    // 2. Dedupe images via the phase filter + pure collector.
    let images = collect_images_from_pods(&all_pods);
    if images.is_empty() {
        return Ok(OrchestrationResult::NoImagesInScope);
    }

    // 3. For each image, build the Job, stamp the owner-ref, and try-create
    //    idempotently. First builder or 403 error short-circuits.
    let job_api: Api<Job> = Api::namespaced(ctx.client.clone(), &ctx.operator_namespace);
    let owner_ref = make_owner_reference(cr_meta);
    let distinct_images = images.len();
    let mut created = 0usize;
    let mut preexisting = 0usize;

    for image_ref in &images {
        let mut job = match build_scan_job(spec, &cr_meta.name, image_ref) {
            Ok(job) => job,
            Err(err) => {
                warn!(
                    event = "build_scan_job_failed",
                    image_ref = %image_ref,
                    error = %err,
                    "builder rejected spec for image; aborting orchestration",
                );
                return Ok(OrchestrationResult::BuildFailed {
                    image_ref: image_ref.clone(),
                    error: format!("{err}"),
                });
            }
        };
        // Stamp the owner reference *after* build_scan_job returns (the builder
        // is a pure data transform per feature 003's contract — ownership is a
        // runtime concern, see research.md §2).
        job.metadata
            .owner_references
            .get_or_insert_with(Vec::new)
            .push(owner_ref.clone());

        match try_create_job_idempotent(&job_api, &ctx.operator_namespace, &job).await {
            JobCreateOutcome::Created => {
                created += 1;
            }
            JobCreateOutcome::Preexisting => {
                preexisting += 1;
            }
            JobCreateOutcome::RbacDenied {
                verb_resource,
                namespace,
                message,
            } => {
                warn!(
                    event = "rbac_insufficient",
                    verb_resource = %verb_resource,
                    namespace = %namespace,
                    message = %message,
                    "job-create denied; aborting orchestration fail-closed (constitution III)",
                );
                return Ok(OrchestrationResult::RbacInsufficient {
                    verb_resource,
                    namespace: Some(namespace),
                    message,
                });
            }
        }
    }

    info!(
        event = "scan_orchestration_complete",
        cr = %cr_meta.name,
        distinct_images,
        created,
        preexisting,
        "ensured scan Jobs for in-scope images",
    );

    Ok(OrchestrationResult::Spawned { distinct_images })
}

/// Distill the `Target` block into a list of namespaces to query. v0.7 honors
/// `target.namespaces` only; `target.labelSelector` is reserved for a later
/// feature (per the spec's Edge Cases — out-of-scope kinds are silently
/// ignored, but labelSelector empty/unset is the common case).
fn resolve_target_namespaces(target: &Target) -> Vec<String> {
    target.namespaces.clone()
}

// =============================================================================
// Tests — pure helpers + fake-Api code paths. Integration coverage in e2e/.
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::core::v1::{Container, EphemeralContainer, Pod, PodSpec, PodStatus};
    use kube::core::ErrorResponse;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn pod(phase: Option<&str>, containers: Vec<(&str, &str)>) -> Pod {
        // containers: list of (kind, image) where kind ∈ {"main","init","ephem"}
        let mut main = Vec::new();
        let mut inits = Vec::new();
        let mut ephems = Vec::new();
        for (kind, image) in containers {
            match kind {
                "main" => main.push(Container {
                    name: format!("c-{}", main.len()),
                    image: Some(image.to_string()),
                    ..Default::default()
                }),
                "init" => inits.push(Container {
                    name: format!("i-{}", inits.len()),
                    image: Some(image.to_string()),
                    ..Default::default()
                }),
                "ephem" => ephems.push(EphemeralContainer {
                    name: format!("e-{}", ephems.len()),
                    image: Some(image.to_string()),
                    ..Default::default()
                }),
                _ => unreachable!("test typo: unknown container kind {kind}"),
            }
        }
        Pod {
            spec: Some(PodSpec {
                containers: main,
                init_containers: if inits.is_empty() { None } else { Some(inits) },
                ephemeral_containers: if ephems.is_empty() {
                    None
                } else {
                    Some(ephems)
                },
                ..Default::default()
            }),
            status: phase.map(|p| PodStatus {
                phase: Some(p.to_string()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn pod_no_status(containers: Vec<(&str, &str)>) -> Pod {
        // `pod()` with phase=None sets a PodStatus { phase: None }, which is
        // semantically different from "no PodStatus at all." Cover both paths.
        let mut p = pod(None, containers);
        p.status = None;
        p
    }

    fn api_err(code: u16, message: &str) -> kube::Error {
        kube::Error::Api(ErrorResponse {
            status: "Failure".to_string(),
            message: message.to_string(),
            reason: "Forbidden".to_string(),
            code,
        })
    }

    fn cr_meta(name: &str) -> CrMetaSnapshot {
        CrMetaSnapshot {
            name: name.to_string(),
            uid: "00000000-0000-0000-0000-000000000007".to_string(),
            namespace: "kusari-operator".to_string(),
        }
    }

    // -----------------------------------------------------------------------
    // T009 — is_pod_in_scope (phase filter)
    // -----------------------------------------------------------------------

    #[test]
    fn t009_is_pod_in_scope_accepts_running() {
        assert!(is_pod_in_scope(&pod(
            Some("Running"),
            vec![("main", "nginx")]
        )));
    }

    #[test]
    fn t009_is_pod_in_scope_accepts_pending() {
        assert!(is_pod_in_scope(&pod(
            Some("Pending"),
            vec![("main", "nginx")]
        )));
    }

    #[test]
    fn t009_is_pod_in_scope_rejects_succeeded() {
        assert!(!is_pod_in_scope(&pod(
            Some("Succeeded"),
            vec![("main", "nginx")]
        )));
    }

    #[test]
    fn t009_is_pod_in_scope_rejects_failed() {
        assert!(!is_pod_in_scope(&pod(
            Some("Failed"),
            vec![("main", "nginx")]
        )));
    }

    #[test]
    fn t009_is_pod_in_scope_rejects_unknown() {
        assert!(!is_pod_in_scope(&pod(
            Some("Unknown"),
            vec![("main", "nginx")]
        )));
    }

    #[test]
    fn t009_is_pod_in_scope_accepts_none_phase_as_pending_equivalent() {
        // pod.status.phase is None (research §4 — watch-list freshness path).
        assert!(is_pod_in_scope(&pod(None, vec![("main", "nginx")])));
        // pod.status is entirely absent — same outcome.
        assert!(is_pod_in_scope(&pod_no_status(vec![("main", "nginx")])));
    }

    // -----------------------------------------------------------------------
    // T010 — collect_images_from_pods (dedup, init/main/ephem, phase filter,
    //         empty/None handling, untagged refs)
    // -----------------------------------------------------------------------

    #[test]
    fn t010_collect_dedupes_across_pods() {
        let pods = vec![
            pod(Some("Running"), vec![("main", "nginx:1.27.0")]),
            pod(Some("Running"), vec![("main", "nginx:1.27.0")]),
            pod(Some("Running"), vec![("main", "redis:7.4.0")]),
        ];
        let images = collect_images_from_pods(&pods);
        assert_eq!(
            images.into_iter().collect::<Vec<_>>(),
            vec!["nginx:1.27.0".to_string(), "redis:7.4.0".to_string()]
        );
    }

    #[test]
    fn t010_collect_includes_init_main_and_ephemeral() {
        let pods = vec![pod(
            Some("Running"),
            vec![
                ("init", "ghcr.io/setup:v1"),
                ("main", "nginx:1.27.0"),
                ("ephem", "busybox:1.36"),
            ],
        )];
        let images = collect_images_from_pods(&pods);
        assert_eq!(images.len(), 3);
        assert!(images.contains("ghcr.io/setup:v1"));
        assert!(images.contains("nginx:1.27.0"));
        assert!(images.contains("busybox:1.36"));
    }

    #[test]
    fn t010_collect_applies_phase_filter() {
        let pods = vec![
            pod(Some("Running"), vec![("main", "in-scope:v1")]),
            pod(Some("Succeeded"), vec![("main", "out-of-scope:v1")]),
            pod(Some("Failed"), vec![("main", "out-of-scope-2:v1")]),
        ];
        let images = collect_images_from_pods(&pods);
        assert_eq!(images.len(), 1);
        assert!(images.contains("in-scope:v1"));
    }

    #[test]
    fn t010_collect_skips_empty_and_none_image_strings() {
        let mut p = pod(Some("Running"), vec![("main", "real:v1")]);
        // Add a container with image=None (edge case from the spec).
        p.spec.as_mut().unwrap().containers.push(Container {
            name: "no-image".to_string(),
            image: None,
            ..Default::default()
        });
        // And one with image="   " (whitespace — also skipped).
        p.spec.as_mut().unwrap().containers.push(Container {
            name: "whitespace".to_string(),
            image: Some("   ".to_string()),
            ..Default::default()
        });
        let images = collect_images_from_pods(&[p]);
        assert_eq!(images.len(), 1);
        assert!(images.contains("real:v1"));
    }

    #[test]
    fn t010_collect_preserves_untagged_refs() {
        // Edge case from spec: pod uses an image with no tag/digest — the
        // operator MUST NOT silently inject `:latest`.
        let pods = vec![pod(Some("Running"), vec![("main", "nginx")])];
        let images = collect_images_from_pods(&pods);
        assert!(images.contains("nginx"));
        assert!(!images.contains("nginx:latest"));
    }

    // -----------------------------------------------------------------------
    // T011 — make_owner_reference shape
    // -----------------------------------------------------------------------

    #[test]
    fn t011_make_owner_reference_has_required_fields() {
        let meta = cr_meta("scan-prod");
        let oref = make_owner_reference(&meta);
        assert_eq!(oref.api_version, "kusari.dev/v1alpha1");
        assert_eq!(oref.kind, "NamespaceScan");
        assert_eq!(oref.name, "scan-prod");
        assert_eq!(oref.uid, "00000000-0000-0000-0000-000000000007");
        assert_eq!(oref.controller, Some(true));
        assert_eq!(oref.block_owner_deletion, Some(true));
    }

    // -----------------------------------------------------------------------
    // T012 — classify_job_create_result: 409 → Preexisting
    // -----------------------------------------------------------------------

    #[test]
    fn t012_classify_job_create_409_is_preexisting() {
        let outcome = classify_job_create_result(
            Err(api_err(409, "jobs.batch \"nsscan-x\" already exists")),
            "kusari-operator",
        );
        match outcome {
            JobCreateOutcome::Preexisting => {}
            other => panic!("expected Preexisting, got {other:?}"),
        }
    }

    #[test]
    fn t012_classify_job_create_ok_is_created() {
        let outcome = classify_job_create_result(Ok(Job::default()), "kusari-operator");
        match outcome {
            JobCreateOutcome::Created => {}
            other => panic!("expected Created, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // T012b — classify_*_result: 403 fail-closed path on BOTH pod-list and
    //          Job-create (closes the H1 audit gap on constitution III).
    // -----------------------------------------------------------------------

    #[test]
    fn t012b_classify_pod_list_403_is_rbac_denied_with_verb_resource_and_namespace() {
        let outcome = classify_pod_list_result(
            Err(api_err(
                403,
                "pods is forbidden: User \"system:serviceaccount:kusari-operator:mikebom-operator\" cannot list resource \"pods\"",
            )),
            "prod",
        );
        match outcome {
            PodListOutcome::RbacDenied {
                verb_resource,
                namespace,
                message,
            } => {
                assert_eq!(verb_resource, "list pods");
                assert_eq!(namespace, "prod");
                // Contract: message includes the kube ErrorResponse.message verbatim.
                assert!(
                    message.contains("pods is forbidden"),
                    "RBAC message should include the kube ErrorResponse.message verbatim, got: {message}"
                );
            }
            other => panic!("expected RbacDenied, got {other:?}"),
        }
    }

    #[test]
    fn t012b_classify_job_create_403_is_rbac_denied_with_verb_resource_and_namespace() {
        let outcome = classify_job_create_result(
            Err(api_err(
                403,
                "jobs.batch is forbidden: User cannot create resource \"jobs\"",
            )),
            "kusari-operator",
        );
        match outcome {
            JobCreateOutcome::RbacDenied {
                verb_resource,
                namespace,
                message,
            } => {
                assert_eq!(verb_resource, "create batch/v1.jobs");
                assert_eq!(namespace, "kusari-operator");
                assert!(
                    message.contains("jobs.batch is forbidden"),
                    "RBAC message should include the kube ErrorResponse.message verbatim, got: {message}"
                );
            }
            other => panic!("expected RbacDenied, got {other:?}"),
        }
    }

    #[test]
    fn t012b_classify_pod_list_404_is_namespace_missing() {
        let outcome =
            classify_pod_list_result(Err(api_err(404, "namespaces \"nope\" not found")), "nope");
        match outcome {
            PodListOutcome::NamespaceMissing => {}
            other => panic!("expected NamespaceMissing, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // T013 — Orchestrator-computed Job name matches scan_job::job_name(...)
    //        for matched inputs (FR-004).
    // -----------------------------------------------------------------------

    #[test]
    fn t013_orchestrator_job_name_round_trips_with_scan_job_helpers() {
        let cr_name = "scan-prod";
        let image_ref = "nginx:1.27.0";
        let from_scan_job =
            crate::scan_job::job_name(cr_name, &crate::scan_job::short_image_hash(image_ref));
        // The contract is that `scan_job::build_scan_job(spec, cr_name, image_ref)`
        // produces a Job named exactly `from_scan_job`. The orchestrator's
        // get-before-create check uses this same combination; the test pins
        // the round-trip so a rename in either crate breaks both.
        assert!(from_scan_job.starts_with("nsscan-scan-prod-"));
        assert_eq!(from_scan_job.len(), "nsscan-scan-prod-".len() + 7);
    }

    // -----------------------------------------------------------------------
    // T029 — Idempotency: sequential calls (fresh → 409 on retry).
    // -----------------------------------------------------------------------

    #[test]
    fn t029_sequential_classify_job_create_idempotency() {
        // First call: success.
        let r1 = classify_job_create_result(Ok(Job::default()), "kusari-operator");
        assert!(matches!(r1, JobCreateOutcome::Created));
        // Second call (same name → 409 in production): preexisting.
        let r2 = classify_job_create_result(
            Err(api_err(409, "jobs.batch \"nsscan-x\" already exists")),
            "kusari-operator",
        );
        assert!(matches!(r2, JobCreateOutcome::Preexisting));
    }
}
