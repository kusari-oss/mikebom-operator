# Phase 1 — Data Model

This feature's "data" is the CRD definition itself, which exists in two representations that must stay in sync.

## 1. Rust source (canonical)

**Location**: `crates/operator/src/crds/namespace_scan.rs`

**Top-level type**: `pub struct NamespaceScanSpec` carries `#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]`. The kube macro generates the kube-aware wrapper `pub struct NamespaceScan` (with `metadata`/`spec`/`status` fields) and the `impl CustomResourceExt for NamespaceScan` that exposes `NamespaceScan::crd() -> CustomResourceDefinition`.

**Sub-types** (all carry `JsonSchema + Serialize + Deserialize`):
| Type | Role |
|------|------|
| `Target` | namespace selector (explicit list or label selector) + kind filter |
| `Schedule` | cron expression or interval duration |
| `ScanFormat` | enum: `CyclonedxJson` \| `Spdx23Json` \| `Spdx3Json` |
| `Output` | backend discriminator + per-backend config |
| `OutputType` | enum: `Pvc` \| `S3` \| `Oci` |
| `PvcOutput`, `S3Output`, `OciOutput` | backend-specific config |
| `NamespaceScanStatus` | status subresource root |
| `StatusCondition`, `ScannedImage` | status children |

No changes to these types in this feature.

**New artifacts in this feature**:
- `crates/operator/src/crds/serialize.rs` adds `pub fn crd_yaml<K: CustomResourceExt>() -> String` — a one-line wrapper around `serde_yaml::to_string(&K::crd()).expect("CRD serialization is infallible")`. The signature is generic so a future second CRD slots in without touching this function.
- `crates/operator/src/lib.rs` adds `pub mod crds; pub mod output; pub mod reconcile; pub mod status;` and the wrapper is reachable as `operator::crds::serialize::crd_yaml::<operator::crds::namespace_scan::NamespaceScan>()`.

## 2. Generated YAML (chart artifact)

**Location**: `charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml`

**Type**: `apiextensions.k8s.io/v1.CustomResourceDefinition`

**Expected top-level shape** (derived; exact field order and contents come from kube + schemars):

```yaml
apiVersion: apiextensions.k8s.io/v1
kind: CustomResourceDefinition
metadata:
  name: namespacescans.kusari.dev
spec:
  group: kusari.dev
  names:
    kind: NamespaceScan
    plural: namespacescans
    shortNames: [nsscan]
    singular: namespacescan
  scope: Namespaced
  versions:
    - name: v1alpha1
      served: true
      storage: true
      schema:
        openAPIV3Schema:
          type: object
          properties:
            spec: {...}     # derived from NamespaceScanSpec
            status: {...}   # derived from NamespaceScanStatus
          required: [spec]
      subresources:
        status: {}
```

The byte-exact content is whatever the generator emits at commit time. The integration test pins it; future changes flow through "regen → check in regenerated YAML → PR".

## 3. Drift check (Rust test code)

**Location**: `crates/operator/tests/crd_drift.rs`

```rust
use operator::crds::namespace_scan::NamespaceScan;
use operator::crds::serialize::crd_yaml;
use pretty_assertions::assert_str_eq;

const CHART_YAML: &str = include_str!(
    "../../../charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml"
);

#[test]
fn chart_crd_yaml_matches_generator() {
    let actual = crd_yaml::<NamespaceScan>();
    assert_str_eq!(
        CHART_YAML.trim_end(),
        actual.trim_end(),
        "Chart CRD YAML drifted from the Rust source. Regenerate with:\n  \
         cargo run --bin mikebom-operator-ctl -- crd \
         --output charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml"
    );
}

#[test]
fn generator_is_deterministic() {
    let a = crd_yaml::<NamespaceScan>();
    let b = crd_yaml::<NamespaceScan>();
    assert_eq!(a, b, "crd_yaml is nondeterministic — investigate before trusting drift check");
}
```

`trim_end` accommodates trailing-newline difference between `serde_yaml` output (no final newline) and `include_str!` (one final newline from file).

## State transitions

N/A. The CRD definition is a static artifact. No lifecycle, no concurrent access, no migration.

## Validation rules sourced from spec

- **FR-004**: byte-identical generator output across runs → enforced by `generator_is_deterministic`.
- **FR-005**: drift check fails build on non-empty diff → enforced by `chart_crd_yaml_matches_generator` running under `cargo test --workspace` in CI.
- **FR-006**: failure message names the regen command → embedded in the `assert_str_eq!` message string above.
- **FR-009**: real generated content in `charts/.../namespacescan.kusari.dev_v1.yaml` → enforced by the test failing if the file is still a placeholder comment (the placeholder won't parse as a CRD and won't match generator output).
