//! Constitution VI E2E for feature 007: in-process integration test against
//! a real kind cluster. Each test constructs fixtures via `kube::Api`,
//! invokes `operator::reconcile::scan_orchestrator::ensure_jobs` (or the
//! full `reconcile()`) directly, and asserts on the resulting cluster state.
//!
//! Unlike `reconciler_skeleton.rs`, no chart install, operator pod, or
//! Docker image build is required — the orchestrator runs *in this test
//! process*, talking to the kind apiserver over kube-rs.
//!
//! Gated behind `MIKEBOM_OPERATOR_E2E=1`. Prerequisite:
//!     `kind create cluster --config e2e/kind-cluster.yaml`
//!
//! Then: `MIKEBOM_OPERATOR_E2E=1 cargo test --test reconciler_spawns_job`
//!
//! Tests:
//! - T021  spawn one Job per distinct in-scope image (excludes Succeeded pod)
//! - T021b 25-image scale within 30s wall-clock budget (FR-011 / SC-001)
//! - T027  status reason transitions to Scanning after spawn
//! - T028  status reason NoImagesInScope when namespace has no in-scope pods
//! - T030  ensure_jobs is idempotent across sequential invocations
//! - T031  ensure_jobs does NOT respawn a Job whose previous run completed

use std::sync::Arc;
use std::time::{Duration, Instant};

use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::{Container, Namespace, Pod, PodSpec, PodStatus};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::{
    api::{DeleteParams, ListParams, Patch, PatchParams, PostParams, PropagationPolicy},
    Api, Client, ResourceExt,
};
use operator::crds::namespace_scan::{
    NamespaceScan, NamespaceScanSpec, Output, OutputType, S3Output, ScanFormat, Schedule, Target,
};
use operator::reconcile::namespace_scan::{reconcile, Ctx};
use operator::reconcile::scan_orchestrator::{ensure_jobs, CrMetaSnapshot, OrchestrationResult};

const OPERATOR_NAMESPACE: &str = "kusari-operator-feature007";

fn e2e_enabled() -> bool {
    std::env::var("MIKEBOM_OPERATOR_E2E").ok().as_deref() == Some("1")
}

fn skip(reason: &str) {
    eprintln!("MIKEBOM_OPERATOR_E2E unset; skipping {reason}");
}

async fn try_client() -> Option<Client> {
    Client::try_default().await.ok()
}

/// One scratch target namespace per test, with a unique suffix to avoid races
/// between concurrent test invocations.
fn unique_ns(prefix: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{prefix}-{nanos:x}").chars().take(50).collect()
}

async fn ensure_namespace(client: &Client, name: &str) {
    let api: Api<Namespace> = Api::all(client.clone());
    let ns = Namespace {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    let _ = api.create(&PostParams::default(), &ns).await;
}

async fn delete_namespace_best_effort(client: &Client, name: &str) {
    let api: Api<Namespace> = Api::all(client.clone());
    let _ = api
        .delete(
            name,
            &DeleteParams {
                propagation_policy: Some(PropagationPolicy::Background),
                ..Default::default()
            },
        )
        .await;
}

/// Construct a `kube::Api`-friendly Pod manifest with the given containers
/// and (optional) status.phase. Container images are passed as plain strings.
fn fixture_pod(name: &str, namespace: &str, images: &[&str], phase: Option<&str>) -> Pod {
    Pod {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(namespace.to_string()),
            ..Default::default()
        },
        spec: Some(PodSpec {
            containers: images
                .iter()
                .enumerate()
                .map(|(i, img)| Container {
                    name: format!("c{i}"),
                    image: Some((*img).to_string()),
                    // Use distroless image substring so the pod can actually
                    // try to start in kind (the operator only enumerates spec.image,
                    // it doesn't wait for ContainerCreating to complete).
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }),
        status: phase.map(|p| PodStatus {
            phase: Some(p.to_string()),
            ..Default::default()
        }),
    }
}

async fn apply_pod(client: &Client, pod: Pod) -> Pod {
    let ns = pod.metadata.namespace.clone().unwrap_or_default();
    let api: Api<Pod> = Api::namespaced(client.clone(), &ns);
    let created = api
        .create(&PostParams::default(), &pod)
        .await
        .expect("create pod");
    // If we asked for status.phase, patch it onto status (status subresource).
    if let Some(want) = pod.status.as_ref().and_then(|s| s.phase.clone()) {
        let patch = serde_json::json!({ "status": { "phase": want } });
        let _ = api
            .patch_status(
                created.metadata.name.as_deref().unwrap(),
                &PatchParams::default(),
                &Patch::Merge(&patch),
            )
            .await;
    }
    created
}

fn s3_spec(target_namespaces: Vec<String>) -> NamespaceScanSpec {
    NamespaceScanSpec {
        target: Target {
            namespaces: target_namespaces,
            kinds: vec![],
            label_selector: None,
        },
        schedule: Schedule {
            cron: Some("0 */6 * * *".to_string()),
            interval: None,
        },
        mikebom_image: "ghcr.io/kusari-oss/mikebom:v0.1.0-alpha.68".to_string(),
        scan_format: ScanFormat::CyclonedxJson,
        output: Output {
            backend_type: OutputType::S3,
            pvc: None,
            s3: Some(S3Output {
                bucket: "test-sboms".to_string(),
                region: "us-west-2".to_string(),
                path_prefix: None,
                credentials_secret_name: Some("aws-creds".to_string()),
            }),
            oci: None,
        },
    }
}

async fn apply_cr(
    client: &Client,
    cr_namespace: &str,
    cr_name: &str,
    spec: NamespaceScanSpec,
) -> NamespaceScan {
    let api: Api<NamespaceScan> = Api::namespaced(client.clone(), cr_namespace);
    let cr = NamespaceScan {
        metadata: ObjectMeta {
            name: Some(cr_name.to_string()),
            namespace: Some(cr_namespace.to_string()),
            ..Default::default()
        },
        spec,
        status: None,
    };
    api.create(&PostParams::default(), &cr)
        .await
        .expect("create CR")
}

async fn list_owned_jobs(client: &Client, cr_name: &str) -> Vec<Job> {
    let api: Api<Job> = Api::namespaced(client.clone(), OPERATOR_NAMESPACE);
    api.list(&ListParams::default().labels(&format!("kusari.dev/namespace-scan={cr_name}")))
        .await
        .map(|l| l.items)
        .unwrap_or_default()
}

async fn cleanup_cr_jobs(client: &Client, cr_name: &str) {
    let api: Api<Job> = Api::namespaced(client.clone(), OPERATOR_NAMESPACE);
    for job in list_owned_jobs(client, cr_name).await {
        let name = job.metadata.name.unwrap();
        let _ = api
            .delete(
                &name,
                &DeleteParams {
                    propagation_policy: Some(PropagationPolicy::Background),
                    ..Default::default()
                },
            )
            .await;
    }
}

// ---------------------------------------------------------------------------
// T021 — Spawn one Job per distinct in-scope image (Succeeded pod excluded).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn t021_spawns_one_job_per_distinct_in_scope_image() {
    if !e2e_enabled() {
        skip("kind-based feature 007 E2E (T021)");
        return;
    }
    let Some(client) = try_client().await else {
        skip("kube client unavailable");
        return;
    };

    let target_ns = unique_ns("f007-t021");
    ensure_namespace(&client, &target_ns).await;
    ensure_namespace(&client, OPERATOR_NAMESPACE).await;

    let _p1 = apply_pod(
        &client,
        fixture_pod("alpha", &target_ns, &["nginx:1.27.0"], Some("Running")),
    )
    .await;
    let _p2 = apply_pod(
        &client,
        fixture_pod(
            "beta",
            &target_ns,
            &["nginx:1.27.0"], // duplicate of alpha's image — should dedupe
            Some("Running"),
        ),
    )
    .await;
    let _p3 = apply_pod(
        &client,
        fixture_pod("gamma", &target_ns, &["redis:7.4.0"], Some("Running")),
    )
    .await;
    let _excluded = apply_pod(
        &client,
        fixture_pod(
            "delta-completed",
            &target_ns,
            &["postgres:16"], // would be a 3rd image if Succeeded weren't excluded
            Some("Succeeded"),
        ),
    )
    .await;

    let cr_name = format!("scan-t021-{}", &target_ns[5..]);
    let cr = apply_cr(
        &client,
        OPERATOR_NAMESPACE,
        &cr_name,
        s3_spec(vec![target_ns.clone()]),
    )
    .await;

    let ctx = Ctx {
        client: client.clone(),
        operator_namespace: OPERATOR_NAMESPACE.to_string(),
    };
    let cr_meta = CrMetaSnapshot {
        name: cr.name_any(),
        uid: cr.metadata.uid.clone().unwrap(),
        namespace: OPERATOR_NAMESPACE.to_string(),
    };

    let result = ensure_jobs(&cr.spec, &cr_meta, &ctx)
        .await
        .expect("ensure_jobs");
    assert!(
        matches!(result, OrchestrationResult::Spawned { distinct_images: 2 }),
        "expected 2 distinct images (postgres excluded by Succeeded phase), got {result:?}",
    );

    let jobs = list_owned_jobs(&client, &cr_name).await;
    assert_eq!(jobs.len(), 2, "expected 2 owned Jobs, got {}", jobs.len());

    // FR-005: every Job has the CR as its controller owner.
    for job in &jobs {
        let orefs = job.metadata.owner_references.as_deref().unwrap_or_default();
        assert_eq!(orefs.len(), 1, "expected exactly one owner ref");
        let oref = &orefs[0];
        assert_eq!(oref.kind, "NamespaceScan");
        assert_eq!(oref.name, cr_name);
        assert_eq!(oref.uid, cr.metadata.uid.clone().unwrap());
        assert_eq!(oref.controller, Some(true));
        assert_eq!(oref.block_owner_deletion, Some(true));
    }

    // cleanup
    cleanup_cr_jobs(&client, &cr_name).await;
    let crs: Api<NamespaceScan> = Api::namespaced(client.clone(), OPERATOR_NAMESPACE);
    let _ = crs.delete(&cr_name, &DeleteParams::default()).await;
    delete_namespace_best_effort(&client, &target_ns).await;
}

// ---------------------------------------------------------------------------
// T021b — 25-image scale within 30s wall-clock (closes M1 gap on FR-011).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn t021b_spawns_at_scale_within_30s() {
    if !e2e_enabled() {
        skip("kind-based feature 007 E2E (T021b scale)");
        return;
    }
    let Some(client) = try_client().await else {
        skip("kube client unavailable");
        return;
    };

    let target_ns = unique_ns("f007-t021b");
    ensure_namespace(&client, &target_ns).await;
    ensure_namespace(&client, OPERATOR_NAMESPACE).await;

    // 25 distinct images via programmatic generation.
    let images: Vec<String> = (1..=25)
        .map(|i| format!("registry.local/test/image-{i}:v1"))
        .collect();
    for (i, img) in images.iter().enumerate() {
        let _ = apply_pod(
            &client,
            fixture_pod(
                &format!("scale-pod-{i:02}"),
                &target_ns,
                &[img.as_str()],
                Some("Running"),
            ),
        )
        .await;
    }

    let cr_name = format!("scan-t021b-{}", &target_ns[6..]);
    let cr = apply_cr(
        &client,
        OPERATOR_NAMESPACE,
        &cr_name,
        s3_spec(vec![target_ns.clone()]),
    )
    .await;

    let ctx = Ctx {
        client: client.clone(),
        operator_namespace: OPERATOR_NAMESPACE.to_string(),
    };
    let cr_meta = CrMetaSnapshot {
        name: cr.name_any(),
        uid: cr.metadata.uid.clone().unwrap(),
        namespace: OPERATOR_NAMESPACE.to_string(),
    };

    let start = Instant::now();
    let result = ensure_jobs(&cr.spec, &cr_meta, &ctx)
        .await
        .expect("ensure_jobs");
    let elapsed = start.elapsed();

    assert!(
        matches!(
            result,
            OrchestrationResult::Spawned {
                distinct_images: 25
            }
        ),
        "expected 25 distinct images, got {result:?}",
    );
    assert!(
        elapsed < Duration::from_secs(30),
        "FR-011: orchestration MUST complete within 30s for 25 images; took {elapsed:?}",
    );

    cleanup_cr_jobs(&client, &cr_name).await;
    let crs: Api<NamespaceScan> = Api::namespaced(client.clone(), OPERATOR_NAMESPACE);
    let _ = crs.delete(&cr_name, &DeleteParams::default()).await;
    delete_namespace_best_effort(&client, &target_ns).await;
}

// ---------------------------------------------------------------------------
// T027 — status reason transitions to Scanning after spawn.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn t027_status_reason_transitions_to_scanning_after_spawn() {
    if !e2e_enabled() {
        skip("kind-based feature 007 E2E (T027)");
        return;
    }
    let Some(client) = try_client().await else {
        skip("kube client unavailable");
        return;
    };

    let target_ns = unique_ns("f007-t027");
    ensure_namespace(&client, &target_ns).await;
    ensure_namespace(&client, OPERATOR_NAMESPACE).await;
    let _ = apply_pod(
        &client,
        fixture_pod("alpha", &target_ns, &["nginx:1.27.0"], Some("Running")),
    )
    .await;

    let cr_name = format!("scan-t027-{}", &target_ns[5..]);
    let cr = apply_cr(
        &client,
        OPERATOR_NAMESPACE,
        &cr_name,
        s3_spec(vec![target_ns.clone()]),
    )
    .await;

    let ctx = Arc::new(Ctx {
        client: client.clone(),
        operator_namespace: OPERATOR_NAMESPACE.to_string(),
    });

    reconcile(Arc::new(cr), ctx).await.expect("reconcile");

    // Re-fetch the CR; status should now report `Scanning`.
    let crs: Api<NamespaceScan> = Api::namespaced(client.clone(), OPERATOR_NAMESPACE);
    let refreshed = crs.get_status(&cr_name).await.expect("get_status");
    let cond = refreshed
        .status
        .as_ref()
        .and_then(|s| s.conditions.iter().find(|c| c.condition_type == "Ready"))
        .expect("Ready condition present");
    assert_eq!(cond.reason.as_deref(), Some("Scanning"), "got: {cond:?}");
    assert_eq!(cond.status, "False");

    cleanup_cr_jobs(&client, &cr_name).await;
    let _ = crs.delete(&cr_name, &DeleteParams::default()).await;
    delete_namespace_best_effort(&client, &target_ns).await;
}

// ---------------------------------------------------------------------------
// T028 — status reason NoImagesInScope for empty namespace.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn t028_status_reason_no_images_in_scope_for_empty_namespace() {
    if !e2e_enabled() {
        skip("kind-based feature 007 E2E (T028)");
        return;
    }
    let Some(client) = try_client().await else {
        skip("kube client unavailable");
        return;
    };

    let target_ns = unique_ns("f007-t028");
    ensure_namespace(&client, &target_ns).await;
    ensure_namespace(&client, OPERATOR_NAMESPACE).await;
    // No pods applied — namespace is empty by construction.

    let cr_name = format!("scan-t028-{}", &target_ns[5..]);
    let cr = apply_cr(
        &client,
        OPERATOR_NAMESPACE,
        &cr_name,
        s3_spec(vec![target_ns.clone()]),
    )
    .await;

    let ctx = Arc::new(Ctx {
        client: client.clone(),
        operator_namespace: OPERATOR_NAMESPACE.to_string(),
    });
    reconcile(Arc::new(cr), ctx).await.expect("reconcile");

    let crs: Api<NamespaceScan> = Api::namespaced(client.clone(), OPERATOR_NAMESPACE);
    let refreshed = crs.get_status(&cr_name).await.expect("get_status");
    let cond = refreshed
        .status
        .as_ref()
        .and_then(|s| s.conditions.iter().find(|c| c.condition_type == "Ready"))
        .expect("Ready condition present");
    assert_eq!(
        cond.reason.as_deref(),
        Some("NoImagesInScope"),
        "got: {cond:?}"
    );

    // And no Jobs should have been created.
    assert!(list_owned_jobs(&client, &cr_name).await.is_empty());

    let _ = crs.delete(&cr_name, &DeleteParams::default()).await;
    delete_namespace_best_effort(&client, &target_ns).await;
}

// ---------------------------------------------------------------------------
// T030 — ensure_jobs is idempotent across sequential invocations.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn t030_ensure_jobs_is_idempotent_across_invocations() {
    if !e2e_enabled() {
        skip("kind-based feature 007 E2E (T030)");
        return;
    }
    let Some(client) = try_client().await else {
        skip("kube client unavailable");
        return;
    };

    let target_ns = unique_ns("f007-t030");
    ensure_namespace(&client, &target_ns).await;
    ensure_namespace(&client, OPERATOR_NAMESPACE).await;
    let _ = apply_pod(
        &client,
        fixture_pod("alpha", &target_ns, &["nginx:1.27.0"], Some("Running")),
    )
    .await;
    let _ = apply_pod(
        &client,
        fixture_pod("beta", &target_ns, &["redis:7.4.0"], Some("Running")),
    )
    .await;

    let cr_name = format!("scan-t030-{}", &target_ns[5..]);
    let cr = apply_cr(
        &client,
        OPERATOR_NAMESPACE,
        &cr_name,
        s3_spec(vec![target_ns.clone()]),
    )
    .await;

    let ctx = Ctx {
        client: client.clone(),
        operator_namespace: OPERATOR_NAMESPACE.to_string(),
    };
    let cr_meta = CrMetaSnapshot {
        name: cr.name_any(),
        uid: cr.metadata.uid.clone().unwrap(),
        namespace: OPERATOR_NAMESPACE.to_string(),
    };

    let r1 = ensure_jobs(&cr.spec, &cr_meta, &ctx)
        .await
        .expect("first ensure_jobs");
    let after_first = list_owned_jobs(&client, &cr_name).await.len();

    let r2 = ensure_jobs(&cr.spec, &cr_meta, &ctx)
        .await
        .expect("second ensure_jobs");
    let after_second = list_owned_jobs(&client, &cr_name).await.len();

    assert_eq!(
        r1, r2,
        "OrchestrationResult must match across idempotent calls"
    );
    assert_eq!(
        after_first, after_second,
        "Job count must not grow on a second call against unchanged cluster state",
    );
    assert_eq!(after_first, 2);

    cleanup_cr_jobs(&client, &cr_name).await;
    let crs: Api<NamespaceScan> = Api::namespaced(client.clone(), OPERATOR_NAMESPACE);
    let _ = crs.delete(&cr_name, &DeleteParams::default()).await;
    delete_namespace_best_effort(&client, &target_ns).await;
}

// ---------------------------------------------------------------------------
// T031 — ensure_jobs does NOT respawn a Job whose previous run completed.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn t031_ensure_jobs_does_not_respawn_for_completed_job() {
    if !e2e_enabled() {
        skip("kind-based feature 007 E2E (T031)");
        return;
    }
    let Some(client) = try_client().await else {
        skip("kube client unavailable");
        return;
    };

    let target_ns = unique_ns("f007-t031");
    ensure_namespace(&client, &target_ns).await;
    ensure_namespace(&client, OPERATOR_NAMESPACE).await;
    let _ = apply_pod(
        &client,
        fixture_pod("alpha", &target_ns, &["nginx:1.27.0"], Some("Running")),
    )
    .await;

    let cr_name = format!("scan-t031-{}", &target_ns[5..]);
    let cr = apply_cr(
        &client,
        OPERATOR_NAMESPACE,
        &cr_name,
        s3_spec(vec![target_ns.clone()]),
    )
    .await;

    let ctx = Ctx {
        client: client.clone(),
        operator_namespace: OPERATOR_NAMESPACE.to_string(),
    };
    let cr_meta = CrMetaSnapshot {
        name: cr.name_any(),
        uid: cr.metadata.uid.clone().unwrap(),
        namespace: OPERATOR_NAMESPACE.to_string(),
    };

    // First call: 1 Job spawned.
    let _ = ensure_jobs(&cr.spec, &cr_meta, &ctx)
        .await
        .expect("first ensure_jobs");
    let initial = list_owned_jobs(&client, &cr_name).await;
    assert_eq!(initial.len(), 1);
    let job_name = initial[0].metadata.name.clone().unwrap();

    // Patch the Job to look completed.
    let jobs: Api<Job> = Api::namespaced(client.clone(), OPERATOR_NAMESPACE);
    let patch = serde_json::json!({
        "status": { "succeeded": 1, "completionTime": chrono::Utc::now().to_rfc3339() }
    });
    let _ = jobs
        .patch_status(&job_name, &PatchParams::default(), &Patch::Merge(&patch))
        .await;

    // Second call: should observe the existing Job (via name collision / 409
    // path) and NOT create a new one. Acceptance Scenario 3.2 in spec.md.
    let _ = ensure_jobs(&cr.spec, &cr_meta, &ctx)
        .await
        .expect("second ensure_jobs");
    let after = list_owned_jobs(&client, &cr_name).await;
    assert_eq!(
        after.len(),
        1,
        "completed Job must not trigger a respawn (would explode Job count over time)",
    );

    cleanup_cr_jobs(&client, &cr_name).await;
    let crs: Api<NamespaceScan> = Api::namespaced(client.clone(), OPERATOR_NAMESPACE);
    let _ = crs.delete(&cr_name, &DeleteParams::default()).await;
    delete_namespace_best_effort(&client, &target_ns).await;
}
