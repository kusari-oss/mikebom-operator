//! `NamespaceScan` CRD per plan §5 (CRD shape v0.1 minimum).
//!
//! apiVersion: kusari.dev/v1alpha1
//! kind: NamespaceScan

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "kusari.dev",
    version = "v1alpha1",
    kind = "NamespaceScan",
    namespaced,
    status = "NamespaceScanStatus",
    shortname = "nsscan"
)]
#[serde(rename_all = "camelCase")]
pub struct NamespaceScanSpec {
    pub target: Target,
    pub schedule: Schedule,
    /// Pinned mikebom image (e.g., `ghcr.io/kusari-oss/mikebom:v0.1.0-alpha.51`).
    pub mikebom_image: String,
    pub scan_format: ScanFormat,
    pub output: Output,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub namespaces: Vec<String>,
    /// Workload kinds. Defaults to `[Pod]` per plan §5 note.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_selector: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    /// Go-style duration string (`6h`, `30m`, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
pub enum ScanFormat {
    #[serde(rename = "cyclonedx-json")]
    CyclonedxJson,
    #[serde(rename = "spdx-2.3-json")]
    Spdx23Json,
    #[serde(rename = "spdx-3-json")]
    Spdx3Json,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Output {
    #[serde(rename = "type")]
    pub backend_type: OutputType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pvc: Option<PvcOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub s3: Option<S3Output>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oci: Option<OciOutput>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OutputType {
    Pvc,
    S3,
    Oci,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PvcOutput {
    pub claim_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_prefix: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct S3Output {
    pub bucket: String,
    pub region: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_prefix: Option<String>,
    /// Name of a Kubernetes Secret in the operator's namespace that holds AWS
    /// credentials. The Secret MUST contain at minimum the keys
    /// `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`. The output-upload
    /// container reads them via `envFrom: { secretRef }`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials_secret_name: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OciOutput {
    pub registry: String,
    pub repository: String,
    /// Name of a Kubernetes Secret of type `kubernetes.io/dockerconfigjson` in
    /// the operator's namespace. The output-upload container mounts this Secret
    /// at `/docker-config/config.json` (via the standard `.dockerconfigjson`
    /// key) and exports `DOCKER_CONFIG=/docker-config` so `oras` (and any other
    /// docker-config-aware tool) finds the credentials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials_secret_name: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NamespaceScanStatus {
    #[serde(default)]
    pub conditions: Vec<StatusCondition>,
    /// Wallclock of the most recent reconcile attempt. Refreshed every reconcile
    /// cycle even when no status transition occurred. Distinct from
    /// `last_scan_completed_at`, which is reserved for actual scan completion
    /// in feature 003+.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reconciled_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_scan_completed_at: Option<String>,
    #[serde(default)]
    pub scanned_images: Vec<ScannedImage>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StatusCondition {
    #[serde(rename = "type")]
    pub condition_type: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_transition_time: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScannedImage {
    pub image_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_sha: Option<String>,
    pub sbom_location: String,
    pub completed_at: String,
}
