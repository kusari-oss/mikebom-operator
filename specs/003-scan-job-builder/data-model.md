# Phase 1 — Data Model

This feature is a pure data transformation: `NamespaceScanSpec + image_ref → batch::v1::Job`. The "model" is the shape of the returned Job and the small in-process types used to construct it.

## 1. Inputs

| Input | Type | Source |
|-------|------|--------|
| `spec` | `&operator::crds::namespace_scan::NamespaceScanSpec` | Existing CRD type from feature 001 |
| `image_ref` | `&str` | Caller-supplied; tag-pinned (`nginx:1.27.0`) or digest-pinned (`nginx@sha256:…`) |
| `cr_name` | `&str` | The owning `NamespaceScan`'s `metadata.name` |

## 2. Outputs

```rust
pub fn build_scan_job(
    spec: &NamespaceScanSpec,
    cr_name: &str,
    image_ref: &str,
) -> Result<k8s_openapi::api::batch::v1::Job, BuildScanJobError>;
```

The returned `Job` carries:

```yaml
apiVersion: batch/v1
kind: Job
metadata:
  name: nsscan-<sanitized-cr-name>-<7-char-hash>   # FR-001, FR-009
  labels:
    app.kubernetes.io/name: mikebom-operator
    app.kubernetes.io/component: scan-job
    kusari.dev/namespace-scan: <cr_name>
    kusari.dev/image-ref-hash: <7-char-hash>
spec:
  completions: 1                                   # FR-007
  parallelism: 1                                   # FR-007
  backoffLimit: 2                                  # FR-007 (≤ 3)
  ttlSecondsAfterFinished: 3600                    # FR-008 (≤ 1 hour)
  template:
    spec:
      restartPolicy: Never                         # FR-007
      volumes:
        - name: workdir
          emptyDir: {}                             # FR-003
      initContainers: []                           # init-pull is in `containers`, NOT initContainers
      containers:
        - name: init-pull                          # FR-002, FR-004
          image: cgr.dev/chainguard/skopeo@sha256:<digest>   # FR-011
          env:
            - name: IMAGE_REF
              value: <image_ref>
          args: ["sh", "-c", "skopeo copy ... && tar -x ..."]
          volumeMounts:
            - name: workdir
              mountPath: /workdir                  # FR-003
        - name: mikebom-scan                       # FR-002, FR-005
          image: <spec.mikebomImage>               # FR-005, II (USE not EMBED)
          args:
            - "sbom"
            - "scan"
            - "--path"
            - "/workdir/rootfs"
            - "--format"
            - <scanFormat>                         # cyclonedx-json | spdx-2.3-json | spdx-3-json
            - "--output"
            - "<scanFormat>=/workdir/out/<short-hash>.<ext>"
          resources:
            requests:
              cpu: "100m"                          # FR-010, research §R4
              memory: "128Mi"
          volumeMounts:
            - name: workdir
              mountPath: /workdir
        - name: output-upload                      # FR-002, FR-006
          image: cgr.dev/chainguard/busybox@sha256:<digest>  # FR-011, Clarifications Q1
          args:
            - "sh"
            - "-c"
            - "ls -la /workdir/out/ && cat /workdir/out/*.json"
          volumeMounts:
            - name: workdir
              mountPath: /workdir
```

**Note**: The bootstrap plan §3 originally referred to `init-pull` as the first phase but Kubernetes' `initContainers` semantics (run sequentially, completion required before main containers start) doesn't fit — the three containers actually share data via the `emptyDir` volume in the *same* pod execution. We use regular `containers` for all three, relying on the ordering of file production: `mikebom-scan` waits via `until [ -d /workdir/rootfs ]; do sleep 1; done` if needed (a research item already noted in §R3's flatten command may be insufficient).

Actually — closer reading: with all three in `containers`, Kubernetes runs them in parallel. That breaks the data-flow contract (init-pull → mikebom-scan → output-upload). We MUST use `initContainers` for init-pull and possibly mikebom-scan, with output-upload alone in `containers`. Revised:

```yaml
spec:
  template:
    spec:
      restartPolicy: Never
      initContainers:
        - name: init-pull          # runs to completion first
          ...
        - name: mikebom-scan       # runs to completion second (initContainers run sequentially)
          ...
      containers:
        - name: output-upload      # runs last; pod terminates when this exits
          ...
```

This matches the bootstrap plan §3 lifecycle. The FRs still hold ("exactly three containers in the pod template" — `initContainers` + `containers` together count as three); we update FR-002's interpretation in the test assertions.

## 3. `BuildScanJobError`

```rust
#[derive(Debug, thiserror::Error)]
pub enum BuildScanJobError {
    #[error("spec.mikebomImage is empty or whitespace-only")]
    EmptyMikebomImage,
    #[error("image_ref is empty or whitespace-only")]
    EmptyImageRef,
}
```

Two narrow failure modes per research §R8. Future features may add variants without breaking existing match arms (enum is `non_exhaustive`? — punt for now; add `#[non_exhaustive]` if/when a 2nd type of error class lands).

## 4. Compile-time constants

```rust
mod defaults {
    pub const INIT_PULL_IMAGE: &str =
        "cgr.dev/chainguard/skopeo@sha256:<digest-resolved-at-T005>";
    pub const OUTPUT_UPLOAD_IMAGE: &str =
        "cgr.dev/chainguard/busybox@sha256:<digest-resolved-at-T005>";
    pub const TTL_SECONDS_AFTER_FINISHED: i32 = 3600;
    pub const BACKOFF_LIMIT: i32 = 2;
    pub const SCAN_CPU_REQUEST: &str = "100m";
    pub const SCAN_MEMORY_REQUEST: &str = "128Mi";
}
```

## 5. Validation rules sourced from spec

| FR | Encoded in | Test |
|----|------------|------|
| FR-001 | `metadata.name` construction in `build_scan_job` + `name_from_cr_and_image` helper | unit: `name_is_dns1123_compliant` |
| FR-002 | `initContainers.len() == 2 && containers.len() == 1`, total = 3 | unit: `pod_template_has_three_containers_in_correct_order` |
| FR-003 | One Volume named `workdir` with `emptyDir`, mounted at `/workdir` by all 3 containers | unit: `all_containers_share_workdir_emptydir` |
| FR-004 | `init-pull` container with skopeo image + extraction args | unit: `init_pull_extracts_rootfs` |
| FR-005 | `mikebom-scan` container uses `spec.mikebomImage` and `mikebom sbom scan` args | unit: `mikebom_scan_uses_spec_image_and_args` |
| FR-006 | `output-upload` container with busybox image + placeholder command | unit: `output_upload_is_v03_placeholder` |
| FR-007 | `restartPolicy: Never`, `completions: 1`, `parallelism: 1`, `backoffLimit ≤ 3` | unit: `job_lifecycle_policies_are_one_shot` |
| FR-008 | `ttlSecondsAfterFinished` in `(0, 3600]` | unit: `ttl_within_one_hour` |
| FR-009 | Job name is deterministic for same `(cr_name, image_ref)`; differs for different `image_ref` | unit: `name_is_deterministic` + `name_differs_for_different_images` |
| FR-010 | `mikebom-scan` container has non-empty `resources.requests` | unit: `mikebom_scan_has_resource_requests` |
| FR-011 | All container images contain `@sha256:` or pinned tag (regex: `(@sha256:[a-f0-9]+\|:[^\s]+$)`) — assert no `:latest` | unit: `all_container_images_are_pinned` |
| FR-012 | Builder returns `Err(BuildScanJobError::EmptyMikebomImage)` when `spec.mikebomImage.trim().is_empty()` | unit: `empty_mikebom_image_errors` |

## 6. State transitions

N/A — pure function.
