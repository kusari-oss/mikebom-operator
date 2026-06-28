# NamespaceScan CRD reference

`apiVersion: kusari.dev/v1alpha1`, `kind: NamespaceScan`, namespaced.

The canonical example is in plan §5 and `examples/namespacescan.yaml`.
The fields below correspond 1:1 to `crates/operator/src/crds/namespace_scan.rs`,
which is the **single source of truth** — constitution principle VII.

## Regenerating the chart CRD YAML

The chart's CRD manifest at `charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml`
is generated from the Rust struct. After editing
`crates/operator/src/crds/namespace_scan.rs`, regenerate:

```sh
cargo run --bin mikebom-operator-ctl -- crd \
  --output charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml
```

A `cargo test --workspace` run verifies the chart YAML matches the generator
(`crates/operator/tests/crd_drift.rs::chart_crd_yaml_matches_generator`). If
the test fails in CI, the failure message will name this regen command
verbatim.

## Spec — top-level fields

| Field | Type | Required | Notes |
|---|---|---|---|
| `target.namespaces` | `[]string` | one of namespaces or labelSelector | explicit list |
| `target.labelSelector` | `string` | one of namespaces or labelSelector | k8s label-selector syntax |
| `target.kinds` | `[]string` | no | defaults to `[Pod]` |
| `schedule.cron` | `string` | one of cron or interval | standard cron expression |
| `schedule.interval` | `string` | one of cron or interval | go-style duration (`6h`, `30m`) |
| `mikebomImage` | `string` | yes | pinned mikebom image tag |
| `scanFormat` | `string` | yes | `cyclonedx-json` \| `spdx-2.3-json` \| `spdx-3-json` |
| `output.type` | `string` | yes | `pvc` \| `s3` \| `oci` |
| `output.pvc` \| `output.s3` \| `output.oci` | object | yes (matching `type`) | backend-specific config |

## Status

| Field | Type | Notes |
|---|---|---|
| `conditions[]` | k8s-style condition objects | exactly one `type=Ready` entry; see condition reasons below |
| `lastReconciledAt` | RFC 3339 string | wallclock of the most recent reconcile attempt; refreshed every reconcile cycle (added in feature 002) |
| `lastScanCompletedAt` | RFC 3339 string | wallclock of the most recent SUCCESSFUL scan completion (feature 003+) |
| `scannedImages[]` | list | per-image record: `imageRef`, `resolvedSha`, `sbomLocation`, `completedAt` (populated by feature 003+) |

### Condition reasons (`status.conditions[type=Ready].reason`)

Values the operator writes in v0.1 (after feature 002):

| `reason`           | `status` | Meaning |
|--------------------|----------|---------|
| `NotYetReconciled` | `False`  | Valid spec; scanning not yet implemented (steady state in v0.1). |
| `InvalidSpec`      | `False`  | `spec.target` has neither namespaces nor a labelSelector. |

Reserved for future features (do not repurpose):

| `reason`            | Introduced in |
|---------------------|---------------|
| `Scanning`          | feature 003 (Job pod template) |
| `ScanFailed`        | feature 003 |
| `ScanCompleted`     | feature 003 (with `status=True`) |
| `RBACInsufficient`  | feature 003+ (per constitution principle III) |
