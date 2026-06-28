# Quickstart: PVC output backend

## For cluster admins wiring a PVC

1. Provision a PVC the operator's Jobs can mount:

```sh
kubectl apply -n kusari-operator -f - <<'YAML'
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: sbom-scratch
spec:
  accessModes: [ReadWriteMany]      # or ReadWriteOnce if you don't run concurrent scans
  resources:
    requests:
      storage: 10Gi
  storageClassName: standard         # or whatever your cluster uses
YAML
```

2. Reference it from your `NamespaceScan`:

```yaml
apiVersion: kusari.dev/v1alpha1
kind: NamespaceScan
metadata:
  name: scan-prod
spec:
  target:
    namespaces: [prod]
  schedule:
    cron: "0 */6 * * *"
  mikebomImage: ghcr.io/kusari-oss/mikebom:v0.1.0-alpha.51
  scanFormat: cyclonedx-json
  output:
    type: pvc
    pvc:
      claimName: sbom-scratch        # required when type=pvc
      pathPrefix: "team-a"           # optional; literal directory name relative to /pvc-output
```

Once the reconciler integration ships (a future feature), SBOMs from scans triggered by this `NamespaceScan` will land at `/<pvc>/team-a/<image-hash>.cdx.json` inside the PVC's filesystem.

### Access modes

| Access mode | When to use |
|-------------|-------------|
| `ReadWriteOnce` (RWO) | Single-node cluster, or only one scan at a time per NamespaceScan |
| `ReadWriteMany` (RWX) | Multi-node, multiple concurrent scans (NFS, CephFS, EFS, etc.) |

The operator does not check access mode — pick what fits your storage class.

## For contributors testing the builder

```sh
# Unit tests — pure-function suite, covers all FRs.
cargo test --workspace --lib operator::scan_job

# Drift check (unchanged from feature 001 — CRD shape didn't change in 004).
cargo test --workspace --test crd_drift

# kind dry-run E2E (gated). Tests the PVC variant manifest validates server-side.
kind create cluster --config e2e/kind-cluster.yaml
MIKEBOM_OPERATOR_E2E=1 cargo test --test scan_job_dryrun
```

## For contributors extending the dispatch (features 005 / 006)

The dispatch surface lives in `crates/operator/src/scan_job/mod.rs`:

```rust
fn build_output_upload_container(output: &Output) -> Result<Container, BuildScanJobError> {
    match &output.backend_type {
        OutputType::Pvc => { /* this feature's PVC arm */ }
        OutputType::S3  => { /* feature 005 lands here */ }
        OutputType::Oci => { /* feature 006 lands here */ }
    }
}
```

To add a new backend:

1. Build a new container helper (e.g., `build_output_upload_s3_container`).
2. Add a match arm in `build_output_upload_container`.
3. Add the new `BuildScanJobError` variant (e.g., `MissingS3Config`) — non-breaking thanks to `#[non_exhaustive]`.
4. If the new backend needs a Job-pod-level volume (like PVC does), thread it through the same conditional-push pattern in `build_scan_job`.
5. Document the new arm in `specs/<your-feature>/contracts/output-backends.md`.

## Common Q&A

| Question | Answer |
|----------|--------|
| Does the operator create the PVC? | No. Admin provisions it before the operator runs. |
| What happens if the PVC doesn't exist when a Job runs? | The pod stays `Pending` (Kubernetes scheduler). The builder doesn't check; it's a runtime concern. |
| Can I use `{namespace}` placeholders in `pathPrefix`? | Not in v0.4 (Clarifications Q1 → A). Pre-template the prefix before applying the CR if you want per-namespace paths. |
| What if I set `pathPrefix: "/foo"` with a leading slash? | The builder strips the single leading `/`. Destination becomes `/pvc-output/foo/`. |
| Can the output-upload container write to other containers' workdir? | Yes — `workdir` is still shared (via emptyDir). The PVC is the only NEW volume; it's mounted only on output-upload. |

## Performance expectations (unchanged from feature 003)

| Operation | Budget |
|-----------|--------|
| `build_scan_job` invocation | < 100µs |
| Full unit-test suite | < 1s (SC-001) |
| Dry-run E2E (per format-PVC variant) | < 5s |
