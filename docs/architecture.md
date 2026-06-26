# Architecture

## USE pattern

The operator does **not** statically link `mikebom`. It orchestrates
ephemeral `batch/v1` Job pods that run the published
`ghcr.io/kusari-oss/mikebom:<tag>` image. This keeps the operator binary
small and decouples release cadence: mikebom continues releasing on
`v*-alpha.*` tag pushes; the operator and its Helm chart version
independently.

See the bootstrap plan (`sparkling-chasing-bee.md` §2 Decision) for the
full rationale and the alternative (a 4th crate in the mikebom repo) that
was rejected.

## Three-container Job choreography

Per plan §3, each scan Job is composed of three containers sharing an
`emptyDir`-backed `/workdir`:

1. **`init-pull`** — `skopeo copy docker://<image-ref> dir:/workdir/image`
   plus a layer-extract helper that flattens the OCI image to
   `/workdir/rootfs`.
2. **`mikebom-scan`** — runs
   `mikebom sbom scan --path /workdir/rootfs --format <format>
    --output <format>=/workdir/out/<sha>.<ext>`.
3. **`output-upload`** — small uploader image that pushes
   `/workdir/out/<sha>.*` to the configured backend (PVC, S3, or OCI).

If `mikebom sbom scan --image <ref>` lands upstream later, this collapses
to a single-container Job. The CRD shape is unaffected.

## Security model

- The operator runs with a tightly-scoped `ClusterRole`: read pods,
  namespaces, and workloads; manage Jobs in target namespaces; manage
  `kusari.dev` resources; manage its own leader-election Lease.
- Scan Jobs run under a separate ServiceAccount with no Kubernetes API
  access — they only need to pull the target image and write output.
- Output credentials (S3, OCI) are mounted from `Secret` references in the
  `NamespaceScan` spec, never from operator-global config.
