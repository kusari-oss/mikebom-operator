# Contract: `build_scan_job`

This documents the stability contract for the public function added in feature 003. Callers (feature 004's reconciler integration, and any future Job-producing code paths) can rely on the shape below.

## Function signature

```rust
pub fn build_scan_job(
    spec: &crate::crds::namespace_scan::NamespaceScanSpec,
    cr_name: &str,
    image_ref: &str,
) -> Result<k8s_openapi::api::batch::v1::Job, BuildScanJobError>;
```

Reachable as `operator::scan_job::build_scan_job`.

## Inputs

- `spec`: the `NamespaceScanSpec` from feature 001. Read-only. The function uses `spec.mikebom_image` and `spec.scan_format`; future versions may use additional fields without breaking the signature.
- `cr_name`: the owning `NamespaceScan`'s `metadata.name`. Used to derive the Job's `metadata.name` and a `kusari.dev/namespace-scan` label.
- `image_ref`: container image reference. Format is the caller's responsibility — anything that resolves under `skopeo copy docker://<image_ref>` works (tag-pinned `nginx:1.27.0`, digest-pinned `nginx@sha256:…`, registry-prefixed `ghcr.io/foo/bar:v1`, etc.).

## Outputs

### Success: `batch::v1::Job`

Stable-contract fields (callers may rely on these):

| Path | Value / shape | Stability |
|------|---------------|-----------|
| `apiVersion` | `"batch/v1"` | locked |
| `kind` | `"Job"` | locked |
| `metadata.name` | `nsscan-<sanitized-cr-name>-<7-char-image-hash>`, ≤ 63 chars, DNS-1123-compliant | format may evolve, name still deterministic for same inputs |
| `metadata.labels` | includes `app.kubernetes.io/name=mikebom-operator`, `app.kubernetes.io/component=scan-job`, `kusari.dev/namespace-scan=<cr_name>`, `kusari.dev/image-ref-hash=<7-char-hash>` | additive — more labels may be added; these will not be removed |
| `spec.template.spec.volumes` | one `emptyDir` volume named `workdir` | locked |
| `spec.template.spec.initContainers[0].name` | `"init-pull"` | locked |
| `spec.template.spec.initContainers[1].name` | `"mikebom-scan"` | locked |
| `spec.template.spec.containers[0].name` | `"output-upload"` | locked for v0.3; feature 004+ may keep this name and change image/command |
| `spec.template.spec.restartPolicy` | `"Never"` | locked |
| `spec.completions` / `parallelism` / `backoffLimit` | `1` / `1` / `≤ 3` | locked at v0.3 values |
| `spec.ttlSecondsAfterFinished` | `(0, 3600]` | range locked; exact value may tune |

Implementation-detail fields (callers MUST NOT depend on these — they may shift across patch releases):

- Init-pull image registry / tag — pinned to digest, refreshed periodically.
- Resource requests on `mikebom-scan` — may tune based on operator experience.
- Args formatting (whitespace, quoting) — semantically stable, byte-shape not.

### Failure: `BuildScanJobError`

| Variant | When |
|---------|------|
| `EmptyMikebomImage` | `spec.mikebom_image` is empty after `trim()` |
| `EmptyImageRef` | `image_ref` is empty after `trim()` |

The error type is `#[non_exhaustive]` (planned) so future variants don't break match arms.

## What this contract does NOT cover

- **Image-pull secret handling**: the Job's `spec.template.spec.imagePullSecrets` is empty; callers wanting registry credentials must set `spec.template.spec.serviceAccountName` via the reconciler (feature 004+).
- **Scheduling hints**: no `nodeSelector`, `tolerations`, `affinity`, or `priorityClassName` set. Callers can add them post-construction; future features may surface these via `NamespaceScanSpec`.
- **Network policies**: out of scope; cluster admin's responsibility.
- **Job ownership**: no `metadata.ownerReferences` set — feature 004's reconciler injects ownership so deletion cascades. Builder is intentionally pure; ownership is a reconciler concern.

## Versioning

- v0.3 (this feature) establishes the contract above.
- Future features (004+) may extend `metadata.labels`, tune resource requests, swap the `output-upload` container's image/command, and ship new container args — all backward-compatible.
- Breaking changes (e.g., renaming a container, changing the volume name) require a major-version bump of the operator AND a documented migration path in the chart.

## Sample output (illustrative — exact digests resolved at impl time)

```yaml
apiVersion: batch/v1
kind: Job
metadata:
  name: nsscan-scan-prod-a3b4c5d
  labels:
    app.kubernetes.io/name: mikebom-operator
    app.kubernetes.io/component: scan-job
    kusari.dev/namespace-scan: scan-prod
    kusari.dev/image-ref-hash: a3b4c5d
spec:
  completions: 1
  parallelism: 1
  backoffLimit: 2
  ttlSecondsAfterFinished: 3600
  template:
    spec:
      restartPolicy: Never
      volumes:
        - name: workdir
          emptyDir: {}
      initContainers:
        - name: init-pull
          image: cgr.dev/chainguard/skopeo@sha256:<digest>
          env:
            - name: IMAGE_REF
              value: nginx:1.27.0
          args:
            - sh
            - -c
            - |
              skopeo copy --src-tls-verify=true docker://$IMAGE_REF dir:/workdir/image \
                && mkdir -p /workdir/rootfs \
                && for layer in /workdir/image/*.tar.gz; do tar -xzf "$layer" -C /workdir/rootfs; done
          volumeMounts:
            - { name: workdir, mountPath: /workdir }
        - name: mikebom-scan
          image: ghcr.io/kusari-oss/mikebom:v0.1.0-alpha.51
          args:
            - sbom
            - scan
            - --path
            - /workdir/rootfs
            - --format
            - cyclonedx-json
            - --output
            - cyclonedx-json=/workdir/out/a3b4c5d.cdx.json
          resources:
            requests:
              cpu: 100m
              memory: 128Mi
          volumeMounts:
            - { name: workdir, mountPath: /workdir }
      containers:
        - name: output-upload
          image: cgr.dev/chainguard/busybox@sha256:<digest>
          args:
            - sh
            - -c
            - ls -la /workdir/out/ && cat /workdir/out/*.json
          volumeMounts:
            - { name: workdir, mountPath: /workdir }
```
