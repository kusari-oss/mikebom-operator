# Phase 0 — Research

Each decision below is a binding input to Phase 1 design.

## R1: PVC volume construction in k8s-openapi

**Decision**: Use `k8s_openapi::api::core::v1::Volume` with `persistent_volume_claim: Some(PersistentVolumeClaimVolumeSource { claim_name, read_only: None })`. The volume name = `"pvc-output"` (deterministic; constants in `defaults` module).

**Rationale**: k8s-openapi exposes the standard structure — `Volume.persistentVolumeClaim` is the canonical Kubernetes spec field. `read_only: None` defaults to false on the K8s side, which is what we want (output-upload writes SBOMs). Hardcoding the volume name keeps lookups deterministic in tests.

**Alternatives considered**:
- Anonymous volume name based on the claim — adds string-construction overhead, complicates test assertions. Rejected.
- `subPath` on the VolumeMount to scope per-image — could work but complicates the cp command's destination logic. Spec FR-003 already handles per-image scoping via `pathPrefix` + the SBOM filename's image-hash prefix. Rejected.

## R2: `output-upload` PVC variant command shape

**Decision**:

```sh
sh -c 'set -eu
DEST="/pvc-output${PATH_PREFIX:+/${PATH_PREFIX}}"
mkdir -p "$DEST"
cp /workdir/out/*.json "$DEST/"'
```

With `PATH_PREFIX` set as a container env var derived from `spec.output.pvc.path_prefix` (after leading-slash strip per FR-008). When `path_prefix` is empty / `None`, `PATH_PREFIX=""` and the destination collapses to `/pvc-output`.

**Rationale**:
- `set -eu` halts on error or unset var — catches a misconfigured `cp` source mid-script.
- `${PATH_PREFIX:+/${PATH_PREFIX}}` is POSIX shell parameter expansion: emits `/<value>` if `PATH_PREFIX` is non-empty, nothing otherwise. Avoids a brittle empty-string-trims-leading-slash hack.
- `mkdir -p` is idempotent — fresh PVC works without admin preparation (FR-009).
- `cp /workdir/out/*.json` relies on shell glob expansion. Busybox `sh` supports globbing.

**Alternatives considered**:
- `find /workdir/out -name '*.json' -exec cp {} "$DEST" \;` — more robust to zero-match (`cp` errors when glob doesn't match), but adds `find` complexity. Rejected; `set -eu` will surface zero-match errors during testing.
- Move `mkdir -p` into Rust by setting it as a `lifecycle.postStart` hook — non-standard for Jobs; rejected.

## R3: `pathPrefix` leading-slash strip

**Decision**: Strip exactly one leading `/` if present:

```rust
fn strip_leading_slash(s: &str) -> &str {
    s.strip_prefix('/').unwrap_or(s)
}
```

Applied before passing the prefix into the `PATH_PREFIX` env var.

**Rationale**: Matches FR-008's "single leading slash strip"; idempotent; preserves the rest of the path (so `"/a/b/c"` becomes `"a/b/c"`). Does NOT strip trailing slashes — those are harmless in `mkdir -p` and the resulting `cp` destination, and stripping would surprise callers who explicitly trailing-slash for clarity.

**Alternatives considered**:
- Strip all leading slashes via `trim_start_matches('/')` — could double-strip `"//"` which is a different unix-path semantic; rejected to match the spec's "strip a single leading `/`" wording exactly.
- Reject leading-slash prefixes with an error — adds a third error variant for a self-healing case; rejected.

## R4: `BuildScanJobError::MissingPvcConfig` variant

**Decision**:

```rust
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum BuildScanJobError {
    #[error("spec.mikebomImage is empty or whitespace-only")]
    EmptyMikebomImage,
    #[error("image_ref is empty or whitespace-only")]
    EmptyImageRef,
    #[error("spec.output.type=Pvc requires spec.output.pvc.claimName to be non-empty")]
    MissingPvcConfig,
}
```

**Rationale**: Variant addition is non-breaking because the enum is `#[non_exhaustive]` (feature 003 §R8). `PartialEq + Eq` derive carries over so test assertions can use `assert_eq!`. The error message names the spec field path the user needs to fix.

**Alternatives considered**:
- Reuse `EmptyMikebomImage` shape with a `MissingField(&'static str)` variant — more flexible but loses pattern-match dispatchability; rejected.
- Wrap in `anyhow::Error` — loses type info; rejected (consistent with feature 003 §R8).

## R5: Unit-test fixture pattern

**Decision**: Extend the existing `valid_spec()` helper in `scan_job::tests` with a sibling `valid_pvc_spec(claim_name: &str, path_prefix: Option<&str>) -> NamespaceScanSpec` that overrides `output.type = Pvc` and populates `output.pvc`. Existing tests using `valid_spec()` continue to exercise the PVC-not-set path (which is now "non-PVC dispatch").

**Rationale**: Symmetric with feature 003's `valid_spec()` / `invalid_spec()` pair. Per-fixture helpers keep tests readable. The "PVC-not-set" path uses the existing fixture, ensuring feature 003's `output_upload_is_v03_placeholder` assertion stays meaningful (now interpretable as "this is what feature 004's dispatch produces when output.type is NOT Pvc").

**Alternatives considered**:
- Parameterize `valid_spec` with an `output: Output` arg — breaks the existing call sites in 12+ tests. Rejected.
- Builder-pattern fixture — over-engineered for 2 use cases.

## R6: Kind dry-run E2E extension

**Decision**: Add a second `#[test] fn pvc_scan_job_passes_server_dry_run` to `e2e/tests/scan_job_dryrun.rs`. Same skopeo… wait, same `kubectl apply --dry-run=server` invocation as feature 003's existing test, with a PVC-fixture spec. Asserts the manifest validates. Loops over the 3 ScanFormat variants for parity.

**Rationale**: Constitution VI is satisfied as long as the kind-cluster API server accepts the manifest. No need to actually create the PVC for `--dry-run=server` — the API server validates the PVC reference syntactically without checking existence.

**Alternatives considered**:
- Single test with parameterization across `(output.type, scan_format)` — could grow to N*M; rejected for clarity; two tests is fine.
- Skip the E2E entirely — violates constitution VI (Job-template construction is in scope).

## R7: Helm chart `values.yaml` example shape

**Decision**: Add a commented `mikebom.output` block to `values.yaml`:

```yaml
mikebom:
  # ...existing...
  output:
    # Default output backend for Helm-managed NamespaceScan templates.
    # The operator does NOT create the PVC — the cluster admin provisions it.
    # Supported types: pvc (S3 in feature 005, OCI in feature 006).
    type: pvc
    pvc:
      claimName: sbom-scratch         # required when type=pvc
      pathPrefix: ""                  # optional; relative to /pvc-output inside the Job pod
```

Plus a sentence in `docs/crd-reference.md` pointing to this example.

**Rationale**: Commented values are the chart's idiomatic way to document optional configuration. Cluster admins reading the chart for the first time see the dispatch surface, the required field, and the operator's no-create stance in one place.

**Alternatives considered**:
- Add a separate `values-example.yaml` — splits the doc surface; rejected.
- Document only in `docs/`, not the chart — chart admins read `values.yaml` first; rejected.

## R8: `output_upload_is_v03_placeholder` test rename / refactor

**Decision**: Rename to `output_upload_non_pvc_is_v03_placeholder` and keep the same assertions; semantics now "when output.type is NOT Pvc, the v0.3 placeholder shape ships unchanged" — which is exactly what FR-004 mandates.

**Rationale**: The test's intent migrates cleanly: feature 003 named it "v0.3 placeholder"; feature 004 reads that as "the dispatch's non-PVC arm produces the v0.3 placeholder". Adding a `pvc_output_upload_copies_to_pvc_mount` test for the PVC arm completes the coverage.

**Alternatives considered**:
- Delete the test — loses regression coverage on the non-PVC path. Rejected.
- Keep the original name — confusing in code review when output-upload's behavior depends on dispatch. Rejected.
