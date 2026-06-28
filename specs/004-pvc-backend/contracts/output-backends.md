# Contract: Output-backend dispatch

This documents the stability contract for how `build_scan_job` dispatches on `spec.output.type` to produce the `output-upload` container's shape and any required volumes. Owned by feature 004; features 005 (S3) and 006 (OCI) extend it.

## Dispatch axis

| `spec.output.backend_type` | `output-upload` container shape | Pod-spec additions |
|----------------------------|---------------------------------|---------------------|
| `Pvc`                      | busybox + real `cp` to `/pvc-output[/<path_prefix>]/` | `persistentVolumeClaim` volume `pvc-output` |
| `S3`                       | v0.3 placeholder (`ls -la /workdir/out/ && cat /workdir/out/*.json`) — feature 005 replaces | (none in v0.4) |
| `Oci`                      | v0.3 placeholder — feature 006 replaces | (none in v0.4) |

## Pvc-dispatch contract

### Inputs read

- `spec.output.pvc.claim_name` — required, non-empty after `.trim()`. If missing or empty, builder returns `BuildScanJobError::MissingPvcConfig`.
- `spec.output.pvc.path_prefix` — optional; leading `/` stripped if present; otherwise passed through verbatim.

### Pod-spec additions

The pod template's `volumes` slice gains exactly one new entry beyond feature 003's `workdir` emptyDir:

```yaml
- name: pvc-output
  persistentVolumeClaim:
    claimName: <spec.output.pvc.claimName>
```

Volume name (`pvc-output`) is locked; tooling can rely on it.

### Container shape

The `output-upload` container's image and volumeMount at `/workdir` are unchanged from feature 003. Two additions:

1. A second `volumeMount` referencing the PVC volume at `/pvc-output`.
2. An `env` entry `PATH_PREFIX` set to the leading-slash-stripped `spec.output.pvc.path_prefix` (empty string if unset).
3. The `command` array changes from the v0.3 placeholder to:

```sh
sh -c 'set -eu
DEST="/pvc-output${PATH_PREFIX:+/${PATH_PREFIX}}"
mkdir -p "$DEST"
cp /workdir/out/*.json "$DEST/"'
```

`PATH_PREFIX` is read from the env, NOT interpolated into the command string at build time — this keeps the builder safe from caller-controlled shell metacharacters.

### Blast-radius limit (FR-007)

The `pvc-output` mount appears ONLY on the `output-upload` container. `init-pull` and `mikebom-scan` continue to mount only `workdir`. If a future feature needs PVC access from another container, that's a contract amendment (this document changes first).

## Failure modes

| Variant | When |
|---------|------|
| `BuildScanJobError::MissingPvcConfig` | `output.type == Pvc` AND (`output.pvc.is_none()` OR `output.pvc.claim_name.trim().is_empty()`) |

Existing variants (`EmptyMikebomImage`, `EmptyImageRef`) still catch their original conditions before dispatch.

## Reserved for features 005 / 006

- **S3 dispatch**: adds a new `output-upload` container shape with `aws s3 cp` (or equivalent). May add env-var-based credential plumbing via `envFrom: secretRef`. Does NOT touch the PVC-dispatch arm.
- **OCI dispatch**: adds another `output-upload` shape using `crane push` or equivalent. May reuse the init-pull container's image (it already ships `crane`).

Both will extend `BuildScanJobError` with additional variants (`MissingS3Config`, `MissingOciConfig`). `#[non_exhaustive]` makes these additions non-breaking.

## What this contract does NOT cover

- Reconciler integration (which CR triggers which Job — out of scope here).
- PVC pre-flight checks (existence, access mode, capacity) — runtime concern.
- Operator-managed PVC creation — admin responsibility (per FR Assumptions).
- Concurrent writes on RWO-mode PVCs — admin's call; builder is agnostic.
