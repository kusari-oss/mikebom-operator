# Phase 1 — Data Model

This feature's "data" is a refinement of feature 003's Job-manifest shape — adding a `persistentVolumeClaim` volume and a real-cp `output-upload` container when `spec.output.type == Pvc`. The error enum gains one variant.

## 1. Inputs (unchanged from feature 003)

Same as `specs/003-scan-job-builder/data-model.md` §1. The builder signature is unchanged:

```rust
pub fn build_scan_job(
    spec: &NamespaceScanSpec,
    cr_name: &str,
    image_ref: &str,
) -> Result<Job, BuildScanJobError>;
```

The new dispatch axis is read from `spec.output.backend_type` (matching the existing `OutputType::{Pvc, S3, Oci}` enum from feature 001).

## 2. Dispatch shape

```rust
fn build_output_upload_container(output: &Output) -> Result<Container, BuildScanJobError> {
    match &output.backend_type {
        OutputType::Pvc => {
            let pvc = output.pvc.as_ref().ok_or(BuildScanJobError::MissingPvcConfig)?;
            if pvc.claim_name.trim().is_empty() {
                return Err(BuildScanJobError::MissingPvcConfig);
            }
            Ok(build_output_upload_pvc_container(pvc))
        }
        OutputType::S3 | OutputType::Oci => {
            // v0.3 placeholder until features 005/006 land.
            Ok(build_output_upload_placeholder_container())
        }
    }
}
```

The Job's pod template's `volumes` slice gains the PVC volume only when `output.type == Pvc`:

```rust
let mut volumes = vec![workdir_volume()];
if let OutputType::Pvc = output.backend_type {
    if let Some(pvc) = &output.pvc {
        if !pvc.claim_name.trim().is_empty() {
            volumes.push(pvc_output_volume(&pvc.claim_name));
        }
    }
}
```

(Both validation paths produce the same `MissingPvcConfig` error if claim_name is empty — caught in `build_output_upload_container` before the volume push happens.)

## 3. PVC volume + mount

```yaml
# Pod template volumes:
- name: workdir
  emptyDir: {}
- name: pvc-output                # NEW in feature 004 (Pvc dispatch only)
  persistentVolumeClaim:
    claimName: <spec.output.pvc.claimName>

# output-upload container volumeMounts (Pvc dispatch):
- name: workdir
  mountPath: /workdir
- name: pvc-output                # NEW
  mountPath: /pvc-output
```

**FR-007 (blast-radius limit)**: the `pvc-output` mount appears ONLY on the `output-upload` container. `init-pull` and `mikebom-scan` continue to mount only `workdir`. Encoded in `build_init_pull_container` / `build_mikebom_scan_container` — neither helper looks at `output`; they're called with `workdir`-only mounts.

## 4. PVC variant `output-upload` container

```yaml
- name: output-upload
  image: cgr.dev/chainguard/busybox@sha256:<digest>     # SAME as v0.3 placeholder (FR-006)
  env:
    - name: PATH_PREFIX
      value: <stripped pathPrefix>                       # leading "/" stripped if present
  command:
    - sh
    - -c
    - |
      set -eu
      DEST="/pvc-output${PATH_PREFIX:+/${PATH_PREFIX}}"
      mkdir -p "$DEST"
      cp /workdir/out/*.json "$DEST/"
  volumeMounts:
    - { name: workdir, mountPath: /workdir }
    - { name: pvc-output, mountPath: /pvc-output }
```

`PATH_PREFIX` is an env var (not embedded in the command string) so the shell does the substitution at runtime and the builder doesn't need to escape user input.

## 5. `BuildScanJobError` (extended)

```rust
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum BuildScanJobError {
    #[error("spec.mikebomImage is empty or whitespace-only")]
    EmptyMikebomImage,
    #[error("image_ref is empty or whitespace-only")]
    EmptyImageRef,
    /// NEW in feature 004
    #[error("spec.output.type=Pvc requires spec.output.pvc.claimName to be non-empty")]
    MissingPvcConfig,
}
```

Match arms in callers (currently none — feature 003 only had the two original variants) won't break because the enum is `#[non_exhaustive]`.

## 6. New defaults

```rust
mod defaults {
    // ... existing from feature 003 ...
    pub const PVC_VOLUME_NAME: &str = "pvc-output";
    pub const PVC_MOUNT_PATH: &str = "/pvc-output";
}
```

## 7. State transitions

N/A — still pure function.

## 8. Validation rules sourced from spec

| FR | Encoded in | Test |
|----|------------|------|
| FR-001 | Pod template's `volumes` slice gains `pvc-output` PV volume when output.type == Pvc | unit: `pvc_dispatch_adds_pvc_volume_to_pod_spec` |
| FR-002 | `output-upload` container's `volume_mounts` gain `pvc-output` mount at `/pvc-output` (Pvc dispatch only) | unit: `pvc_output_upload_mounts_pvc_at_known_path` |
| FR-003 | `output-upload` command includes `mkdir -p` + `cp /workdir/out/*.json` to the PATH_PREFIX-derived destination | unit: `pvc_output_upload_copies_to_pvc_mount` + `pvc_output_upload_respects_path_prefix` |
| FR-004 | Non-Pvc dispatch (S3, Oci) produces the v0.3 placeholder shape; no PVC volume or mount | unit: `output_upload_non_pvc_is_v03_placeholder` (renamed from feature 003) |
| FR-005 | Pvc with `output.pvc = None` OR `claim_name.trim().is_empty()` returns `MissingPvcConfig` | unit: `missing_pvc_config_errors` (two cases: None + empty) |
| FR-006 | All container images stay digest-pinned; output-upload uses the same digest as feature 003 | unit: `all_container_images_are_pinned` (existing, unchanged) |
| FR-007 | `init-pull` and `mikebom-scan` continue to mount only `workdir` (NOT `pvc-output`) | unit: `pvc_volume_mounted_only_on_output_upload` |
| FR-008 | leading-slash strip on `path_prefix` → destination `<pvc-mount>/<stripped>/` | unit: `path_prefix_strips_leading_slash` |
| FR-009 | command includes `mkdir -p "$DEST"` | unit: covered by `pvc_output_upload_copies_to_pvc_mount` (assertion includes "mkdir -p") |
| FR-010 | `charts/mikebom-operator/values.yaml` has commented `mikebom.output` block; `docs/crd-reference.md` has "Output backends" section with PVC example | unit: not test-asserted (docs); validated by review |
| FR-011 | `BuildScanJobError` carries `#[non_exhaustive]` (already inherited from feature 003) | unit: covered by adding the new variant compiling |
| FR-012 | All feature 001/002/003 existing tests pass | unit: `cargo test --workspace` exit code |
