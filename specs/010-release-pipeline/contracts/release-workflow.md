# Contract: release workflow

Internal contract for the `.github/workflows/release.yml` workflow extended by
feature 010. Not a Rust public API, but pinning the inputs / outputs /
failure-mode invariants keeps future signing or attestation work (SLSA-L3, more
artifacts) extending cleanly.

## Workflow inputs

- **`push.tags`**: Filter pattern `v*`. The literal pushed tag is available as
  `${{ github.ref_name }}` and `$GITHUB_REF_NAME` inside steps.
- **`secrets.GITHUB_TOKEN`**: Provided automatically by GitHub Actions.
  Scoped to the workflow's `permissions:` block (per-job opt-in to
  `packages: write` for ghcr.io publish, `contents: write` for the Release
  page).
- **`id-token: write`** permission (per-job): required for cosign keyless
  signing. Generates a short-lived OIDC token that cosign exchanges with
  Fulcio for a signing certificate.
- **No other secrets**: cosign keyless requires zero long-lived credentials.

## Workflow outputs

- **Operator container image**: `ghcr.io/kusari-oss/mikebom-operator:<tag>`
  (multi-arch: linux/amd64 + linux/arm64). Cosign-signed by digest; SBOM
  attestation (CycloneDX) attached.
- **Helm chart**: `oci://ghcr.io/kusari-oss/charts/mikebom-operator:<tag-without-v>`.
  Cosign-signed.
- **GitHub Release page**: at the pushed tag. Body contains artifact links +
  cosign verification commands.
- **Rekor transparency log entries**: public Sigstore Rekor at
  `https://search.sigstore.dev/?logIndex=<n>`. Created automatically by cosign
  sign / cosign attest.

## Job dependency graph

```
versions  ──▶  image  ──▶  chart  ──▶  release-notes
```

- **`versions`** runs first. Verifies the four version strings agree (git tag,
  Cargo.toml workspace version, Chart.yaml `version`, Chart.yaml `appVersion`).
  If any drift, the entire workflow aborts before any push.
- **`image`** depends on `versions`. Builds + pushes multi-arch image
  (existing), then signs the image by digest (new), then generates +
  attaches the SBOM attestation (new).
- **`chart`** depends on `image`. Packages + pushes the chart (existing),
  then signs the chart OCI ref (new).
- **`release-notes`** depends on `chart` (transitively on `image`). Creates
  or updates the GitHub Release page with artifact links + verification
  commands.

## Invariants

1. **Pre-flight gates everything**: no `docker push`, `helm push`, `cosign
   sign`, or GitHub Release creation occurs when the `versions` job fails.
   The four-way agreement (Cargo.toml ↔ Chart.yaml `version` ↔ Chart.yaml
   `appVersion` ↔ tag-without-`v`) is the *only* admission criterion. (FR-009)
2. **Sign-by-digest, not by-tag**: `cosign sign` MUST receive the image's
   `repo@sha256:<digest>` (read from the previous step's output), not the
   `repo:tag` form. This makes the signature immutable against any
   subsequent re-tag of the same digest by an attacker who somehow obtains
   ghcr.io write permissions.
3. **Idempotent re-runs**: re-running the workflow at the same tag (force-
   push, `workflow_dispatch`) MUST succeed. Cosign sign on the same digest
   is a no-op (Rekor de-duplicates the entry). `softprops/action-gh-release`
   updates the existing Release rather than failing on conflict. (FR-014)
4. **Least-privilege permissions**: workflow top-level `permissions:` is
   `contents: read`. Every job that needs more has an explicit `permissions:`
   block listing only what it needs. The new `id-token: write` permission is
   ONLY granted to jobs that invoke cosign. (FR-015)
5. **All actions SHA-pinned**: every `uses: <action>@<ref>` line MUST use a
   40-hex-character commit SHA, with the version tag as a trailing comment.
   Pattern `uses: [^@]*@v[0-9]` MUST NOT appear in the file. (FR-011)
6. **No long-lived signing secrets**: cosign signs via OIDC token →
   short-lived certificate from Fulcio. No PGP/RSA private keys are stored
   in the repo or in GitHub Actions secrets.

## Failure semantics (see also spec.md Edge Cases)

| Job | Failure cause | Outcome | Recovery |
|---|---|---|---|
| `versions` | Cargo.toml / Chart.yaml / tag disagree | Workflow fails, no push | Fix sources, commit, re-tag |
| `image` push | ghcr.io auth or push error | No chart, no release | Re-run workflow (idempotent if root cause fixed) |
| `image` cosign sign | Sigstore Fulcio/Rekor outage | Image pushed but unsigned | Re-run (cosign sign by-digest is idempotent) |
| `image` SBOM attest | Syft scan error OR Rekor outage | Image signed, no SBOM attestation | Re-run; cosign attest is idempotent |
| `chart` push | helm push error | Image up, chart not | Re-run; or `helm push` manually from maintainer machine |
| `chart` cosign sign | Same as image cosign sign | Chart pushed but unsigned | Re-run |
| `release-notes` | gh API error | All artifacts up, no Release page | Re-run; or `gh release create` manually |

## Non-goals (out of scope for feature 010)

- **SLSA-L3 build provenance via `slsa-framework/slsa-github-generator`**:
  cosign + GH OIDC certificate provides L2-equivalent provenance (verifiable
  build identity + source repository). Adding the full SLSA generator is a
  follow-up. Spec Assumptions section documents this.
- **Automated post-publish kind smoke test**: a runbook checklist in
  `docs/release-runbook.md` covers it manually. Automating is a follow-up.
- **Multi-arch beyond amd64+arm64**: linux/s390x, freebsd, etc. — not for v0.1.
- **Automated changelog generation**: release-notes body uses a static
  template populated with tag-specific data, not commit-history aggregation.
- **Manual release approval gate** (e.g., GitHub Environment protection
  rules requiring a second maintainer to approve): not for v0.1; tag-push is
  the implicit authorization.
- **Maintainer key management for non-keyless signing**: would require
  Cosign/PGP key generation + secret storage. Keyless OIDC is the project's
  signing path; explicit-key signing is out of scope.

## Versioning of the workflow itself

This contract describes the workflow as of feature 010. Future features
(SLSA-L3, automated smoke, …) add jobs; renaming any of the existing job
names (`versions`, `image`, `chart`, `release-notes`) is a breaking change to
this contract because external monitoring tools that watch GH Actions runs
key off job names.
