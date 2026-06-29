# Phase 1 Data Model: v0.1.0-alpha.1 release pipeline

Records the artifacts feature 010 produces and the FR → test mapping. No Rust
types are added; the only new Rust artifact is an integration test that reads
existing YAML/TOML files.

## Version-string entity

The single conceptual version is encoded in **three** files in the repository
and **one** ephemeral location at release time (the git tag):

| Source | Field | Example |
|---|---|---|
| `Cargo.toml` | `[workspace.package] version` | `"0.1.0-alpha.1"` |
| `charts/mikebom-operator/Chart.yaml` | `version` | `0.1.0-alpha.1` |
| `charts/mikebom-operator/Chart.yaml` | `appVersion` | `"0.1.0-alpha.1"` |
| Git tag (at release time only) | `GITHUB_REF_NAME` minus `v` prefix | `v0.1.0-alpha.1` → `0.1.0-alpha.1` |

**Invariant**: all four MUST be equal at every release. Enforcement points:

- **PR time**: `crates/operator/tests/version_consistency.rs` integration test
  asserts the three in-repo strings are equal. Runs in every `cargo test
  --workspace` invocation.
- **Release time**: `.github/scripts/check-versions.sh` (invoked by the
  `versions` job) additionally asserts all three match the stripped tag.

## Workflow job graph

```
push:
  tags: [v*]
     │
     ▼
┌───────────────────────────┐
│ versions  (NEW)           │  reads tag + 3 in-repo strings
│  - install yq             │  asserts all match
│  - check-versions.sh      │  fail-fast: no pushes if drift
└─────────┬─────────────────┘
          │ ok
          ▼
┌──────────────────────────────────────┐
│ image (existing, extended)           │  permissions: contents: read,
│  - checkout                          │               packages: write,
│  - setup-qemu                        │               id-token: write (NEW)
│  - setup-buildx                      │
│  - login (ghcr.io)                   │
│  - build-push (multi-arch)           │  → image pushed to ghcr.io
│  - install cosign                NEW │
│  - cosign sign (by digest)        NEW│  → image signed
│  - anchore/sbom-action            NEW│  → SBOM file generated
│  - cosign attest cyclonedx        NEW│  → SBOM attestation attached
└─────────┬────────────────────────────┘
          │ ok
          ▼
┌──────────────────────────────────────┐
│ chart (existing, extended)           │  permissions: contents: read,
│  - checkout                          │               packages: write,
│  - setup-helm                        │               id-token: write (NEW)
│  - helm registry login               │
│  - helm package                      │
│  - helm push                         │  → chart pushed to oci://ghcr.io
│  - install cosign                NEW │
│  - cosign sign (chart OCI ref)    NEW│  → chart signed
└─────────┬────────────────────────────┘
          │ ok
          ▼
┌──────────────────────────────────────┐
│ release-notes (NEW)                  │  permissions: contents: write,
│  - softprops/action-gh-release       │               id-token: read
│    body: image ref + chart ref +     │  → GitHub Release created/updated
│          cosign verify commands +    │
│          SBOM attestation note       │
└──────────────────────────────────────┘
```

**Dependency order**: `versions` → `image` → `chart` → `release-notes`. The
`image` → `chart` ordering is kept from the existing `release.yml` (chart
references the just-published image's tag). `release-notes` depends on both
upstream successes — partial failures mean no Release page until manual
fix-up.

## Output artifacts

| Artifact | Location | Signing | Attestation |
|---|---|---|---|
| Operator image (multi-arch) | `ghcr.io/kusari-oss/mikebom-operator:v<semver>` | cosign keyless OIDC | CycloneDX SBOM attached via cosign attest |
| Helm chart (OCI) | `oci://ghcr.io/kusari-oss/charts/mikebom-operator:<semver>` (without `v`) | cosign keyless OIDC | — |
| GitHub Release page | `https://github.com/kusari-oss/mikebom-operator/releases/tag/v<semver>` | — | — |
| Rekor transparency log entries | Public Sigstore Rekor | — | — (entries created by cosign sign/attest) |

## Failure semantics

| Job | Failure outcome | Recovery |
|---|---|---|
| `versions` | No artifact pushed; workflow exits non-zero with a diff naming the drifted file(s). | Fix the source file (Cargo.toml / Chart.yaml), commit, re-tag. |
| `image` build/push | No chart, no release-notes. Tag retried by re-running the workflow (or force-pushing the tag if a real fix needed). | Re-run the workflow at the same tag (cosign sign is idempotent on same digest). |
| `image` cosign sign / SBOM attest | Image is published but unsigned. Workflow fails loudly. | Re-run; cosign attestation is idempotent. Worst case: manual `cosign sign` from a maintainer machine using `cosign sign-blob` against the image digest. |
| `chart` push | Image is up, chart is not. Workflow fails. | Re-run after diagnosing; OR manually publish from a maintainer machine using `helm push`. Document in runbook. |
| `chart` cosign sign | Chart is published but unsigned. Same as image sign-fail; re-run is idempotent. |
| `release-notes` | All artifacts up, no Release page. Manually create via `gh release create v<semver>`. | Re-run the workflow OR manually create. |

## FR → test mapping

| FR | Test |
|---|---|
| FR-001 (tag triggers workflow) | Existing `on: push: tags: [v*]` filter. Manually verified on a test tag. |
| FR-002 (multi-arch) | Existing `docker/build-push-action` `platforms: linux/amd64,linux/arm64`. No change. |
| FR-003 (image at expected ref) | Existing `tags:` parameter. Post-publish runbook: `docker manifest inspect`. |
| FR-004 (cosign sign image) | New `cosign sign` step. Post-publish runbook: `cosign verify ...`. |
| FR-005 (SBOM attestation) | New `anchore/sbom-action` + `cosign attest` steps. Post-publish runbook: `cosign verify-attestation --type cyclonedx ...`. |
| FR-006 (chart version=appVersion=tag) | `versions` pre-flight job + `crates/operator/tests/version_consistency.rs` integration test. |
| FR-007 (chart pushed to OCI) | Existing `helm push` step. No change. |
| FR-008 (cosign sign chart) | New `cosign sign` step on chart OCI ref. Post-publish runbook: `cosign verify oci://...`. |
| FR-009 (pre-flight version check) | New `versions` job, runs first, gates everything downstream. T-XX grep verifies the job exists with `needs:` dependencies set correctly on `image` + `chart`. |
| FR-010 (GitHub Release page) | New `release-notes` job. Post-publish runbook: `gh release view v<semver>`. |
| FR-011 (all actions SHA-pinned) | New T-XX grep check: `grep -E "uses: [^@]*@v[0-9]" .github/workflows/release.yml` returns 0 matches. |
| FR-012 (Dockerfile base pins intact) | New T-XX grep check: `grep -E "^FROM [^@]*@sha256:" Dockerfile` returns ≥ 2 matches (builder + final stages). |
| FR-013 (15-min budget) | Measured post-tag-push; SC-001. Documented in runbook. |
| FR-014 (idempotent re-runs) | Workflow re-run on the same tag must succeed. Tested via `workflow_dispatch` dry-run mode. |
| FR-015 (least-privilege permissions) | New T-XX grep: top-level `permissions: contents: read`; every job that needs more has explicit `permissions:` block. |

## In-repo Rust artifact (the only Rust change)

`crates/operator/tests/version_consistency.rs`:

```rust
//! Asserts the three in-repo version strings agree.
//! Runs on every `cargo test --workspace` invocation — drift is caught at PR
//! time, before any release attempt.

use std::fs;
use std::path::PathBuf;

#[test]
fn cargo_workspace_version_matches_chart_version_and_appversion() {
    let root = workspace_root();
    let cargo_version = read_cargo_version(&root.join("Cargo.toml"));
    let chart_path = root.join("charts/mikebom-operator/Chart.yaml");
    let (chart_version, chart_app_version) = read_chart_versions(&chart_path);

    assert_eq!(
        cargo_version, chart_version,
        "Cargo.toml workspace version ({cargo_version}) does not match \
         Chart.yaml version ({chart_version})",
    );
    assert_eq!(
        cargo_version, chart_app_version,
        "Cargo.toml workspace version ({cargo_version}) does not match \
         Chart.yaml appVersion ({chart_app_version})",
    );
}

fn workspace_root() -> PathBuf { /* ... walks up from CARGO_MANIFEST_DIR */ }
fn read_cargo_version(_p: &PathBuf) -> String { /* ... grep-style parse */ }
fn read_chart_versions(_p: &PathBuf) -> (String, String) { /* ... serde_yaml parse */ }
```

Approx 50 lines. Uses workspace deps (`serde_yaml`) — no new Cargo deps needed.
