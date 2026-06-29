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

Values the operator writes:

| `reason`           | `status` | Meaning | Introduced in |
|--------------------|----------|---------|---------------|
| `NotYetReconciled` | `False`  | Valid spec; the operator has not yet attempted to orchestrate scan Jobs (transient; only seen on the very first reconcile cycle once feature 007 is live). | feature 002 |
| `InvalidSpec`      | `False`  | `spec.target` has neither `namespaces` nor a `labelSelector`. Fix the CR. | feature 002 |
| `Scanning`         | `False`  | The operator has spawned (or finds preexisting) one scan `batch/v1.Job` per distinct in-scope container image. Steady state for any CR with active workloads. | feature 007 |
| `NoImagesInScope`  | `False`  | Target namespaces resolved to zero pods in phase `Running` or `Pending`. Apply workloads to a target namespace, or wait for the next reconcile cycle. | feature 007 |
| `BuildFailed`      | `False`  | `scan_job::build_scan_job` rejected the CR's `output` block for at least one image (e.g., `output.type=Pvc` without `pvc.claimName`). Message names the failing image. | feature 007 |
| `RBACInsufficient` | `False`  | The operator's ServiceAccount lacks `pods:list` in a target namespace, or `jobs:create` in its own namespace. Fail-closed across the entire CR per constitution III. | feature 007 |
| `ScanCompleted`    | `True`   | All owned scan Jobs have `status.succeeded >= 1`. `status.scannedImages[]` is populated with one entry per scanned image; `status.lastScanCompletedAt` advances. With v0.8, `kubectl wait --for=condition=Ready namespacescan/<name>` exits 0 on this transition. | feature 008 |
| `ScanFailed`       | `False`  | At least one owned scan Job has `status.failed >= backoffLimit + 1` (default 7 failed pods). Failure dominates partial-success: if 1 Job failed and 2 succeeded, the reason is still `ScanFailed`. Message names a failing image so the admin knows where to look. | feature 008 |

No reasons are currently reserved for future features.

## Output backends

`spec.output.type` selects how scan SBOMs leave the Job pod. v0.4 ships the PVC
backend; features 005 and 006 layer S3 and OCI-registry-as-storage on the same
dispatch.

### PVC

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
      claimName: sbom-scratch   # required; PVC must already exist in the operator namespace
      pathPrefix: "team-a"       # optional; literal directory name relative to /pvc-output
```

When the operator's `output-upload` container runs against this CR, it copies
every `/workdir/out/*.json` SBOM produced by `mikebom-scan` to
`/pvc-output/team-a/` inside the PVC's filesystem. With `pathPrefix` unset or
empty, SBOMs land at the PVC mount root.

The operator does **not** create the PVC. Provision it yourself:

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
  storageClassName: standard
YAML
```

Access-mode guidance:

| Access mode | When to use |
|-------------|-------------|
| `ReadWriteOnce` (RWO) | Single-node cluster, or one scan at a time per NamespaceScan |
| `ReadWriteMany` (RWX) | Multi-node cluster running concurrent scans (NFS, CephFS, EFS, etc.) |

### S3

Uploads SBOMs to an Amazon S3 bucket via the `aws s3 cp` command running in the
`output-upload` container. Credentials come from a user-supplied Kubernetes
Secret in the operator's namespace.

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
    type: s3
    s3:
      bucket: sboms-prod
      region: us-west-2
      credentialsSecretName: aws-creds   # required; Secret in operator namespace
      pathPrefix: "team-a"               # optional; literal key prefix under the bucket
```

The referenced Secret must contain at least:

```sh
kubectl create secret generic aws-creds -n kusari-operator \
  --from-literal=AWS_ACCESS_KEY_ID=AKIA… \
  --from-literal=AWS_SECRET_ACCESS_KEY=…
```

The `output-upload` container reads them via `envFrom: { secretRef }`; region
and bucket are populated from the CR as literal env vars (`AWS_REGION`,
`AWS_DEFAULT_REGION`, `S3_BUCKET`, `S3_PATH_PREFIX`).

The destination object key for each SBOM is
`s3://<bucket>/<pathPrefix>/<image-hash>.<format-extension>` (or
`s3://<bucket>/<image-hash>.<format-extension>` when `pathPrefix` is unset).

### OCI

Pushes each SBOM as an OCI artifact tagged with the scanned image's 7-character
hash. Uses [ORAS](https://oras.land/) (a distroless image with no shell —
arguments use Kubernetes `$(VAR)` substitution). Credentials come from a
user-supplied Kubernetes Secret of type `kubernetes.io/dockerconfigjson`.

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
    type: oci
    oci:
      registry: ghcr.io                   # required
      repository: kusari-oss/sboms        # required
      credentialsSecretName: registry-creds  # required
```

Provision the Secret with `kubectl create secret docker-registry`:

```sh
kubectl create secret docker-registry registry-creds -n kusari-operator \
  --docker-server=ghcr.io \
  --docker-username=$GITHUB_USER \
  --docker-password=$GITHUB_TOKEN
```

The `output-upload` container mounts the Secret read-only at `/docker-config/`
(remapped from the standard `.dockerconfigjson` key to `config.json`) and
exports `DOCKER_CONFIG=/docker-config` so `oras` finds the credentials at the
expected path.

Each SBOM lands at:

```
oci://<registry>/<repository>:<image-hash>
```

where `<image-hash>` is the 7-character SHA-256 prefix of the scanned image
ref. The artifact's media type is `application/json` (CycloneDX/SPDX-specific
media types are a future enhancement).
