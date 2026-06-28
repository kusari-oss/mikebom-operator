# Phase 0 — Research

Each decision below is a binding input to Phase 1 design.

## R1: `init-pull` container image

**Decision**: Use Chainguard's distroless `skopeo` image, digest-pinned: `cgr.dev/chainguard/skopeo` at a specific manifest-list digest resolved via `crane digest cgr.dev/chainguard/skopeo:latest` at implementation time (T-005). Pin format: `cgr.dev/chainguard/skopeo@sha256:<digest> # latest as of YYYY-MM-DD`.

**Rationale**: Chainguard's `skopeo` is distroless (smaller attack surface than `quay.io/skopeo/stable` Alpine base), maintained on the same SLSA-3 build pipeline Kusari already trusts, and supports all the `skopeo copy docker://… dir:/workdir/image` operations the bootstrap plan §3 prescribes. Matches the user's preference saved in memory (`feedback_pin_third_party_deps.md`) for pinning to digests + leaving a version comment for human/Renovate readability.

**Alternatives considered**:
- `quay.io/skopeo/stable` — Alpine-based, broader package surface, less aligned with Kusari's distroless preference. Rejected.
- Building a custom layer-flatten image — premature. The `skopeo copy` output is well-defined; standard tar utilities flatten layers. Rejected.

## R2: `output-upload` container image (v0.3 placeholder)

**Decision**: Use Chainguard's distroless `busybox` image, digest-pinned: `cgr.dev/chainguard/busybox@sha256:<digest>`. Command: `["sh", "-c", "ls -la /workdir/out/ && cat /workdir/out/*.json"]`. Resolved at T-005 via `crane digest cgr.dev/chainguard/busybox:latest`.

**Rationale**: Clarifications Q1 → C settled the shape; this decision pins the specific image. busybox provides `sh`, `ls`, and `cat` in ~5MB, all that's needed for the placeholder behavior. Feature 004 replaces image + command in one PR.

**Alternatives considered**: see Clarifications Q1.

## R3: Layer-flatten step in `init-pull`

**Decision**: Use a single shell command chained inside the `init-pull` container's `args`:

```sh
sh -c 'skopeo copy --src-tls-verify=true \
  docker://${IMAGE_REF} dir:/workdir/image \
  && for layer in /workdir/image/*.tar.gz; do \
       tar -xzf "$layer" -C /workdir/rootfs; \
     done'
```

Where `${IMAGE_REF}` is supplied via the container's `env`. `chainguard/skopeo` ships `sh` and `tar` — sufficient for layer extraction.

**Rationale**: Avoids needing a separate flatten-helper image. `chainguard/skopeo` is built on a minimal distroless base that includes `busybox` utilities (verified via `crane export`). Layer order matters (later layers override earlier — including whiteouts), but for SBOM purposes a naive flatten is acceptable: mikebom's scanner reads file paths and content, and the dominant layer wins on duplicate-path files which is what we want.

**Alternatives considered**:
- Use a separate flatten container — adds a 4th container, contradicts FR-002's "exactly 3 containers" requirement.
- Use `umoci` or `crane export` — additional dependency, distroless-incompatible.

**Trade-off acknowledged**: whiteout-file handling (`.wh.*` entries deleting files in a parent layer) is NOT correctly modeled by naive `tar -x`. Files that should be deleted by a later layer's whiteout will appear in the flat rootfs. This is acceptable for v0.3 because SBOM scanners care about packages, not exact file presence; whiteout-aware extraction lands as a future refinement (separate feature; non-blocking).

## R4: `mikebom-scan` container resource requests

**Decision**: `requests: { cpu: "100m", memory: "128Mi" }`, no limits. Encoded as Rust `Quantity::from("100m")` / `Quantity::from("128Mi")`.

**Rationale**: Conservative defaults that pass scheduling on most kind / dev / small-prod clusters without complaint. mikebom's scan workload is mostly I/O (reading files) with brief CPU bursts for hashing — 100m baseline lets the burst use whatever CPU's free, and 128Mi is enough for the scanner's working set (mikebom's own bins + libstd + a few MB of in-memory file metadata). No limits because killing a scan mid-flight for memory pressure produces a partial SBOM that's worse than a delayed one; scheduler back-pressure and node-level OOM are the safety net.

**Alternatives considered**:
- `200m / 256Mi` — slightly higher; reasonable but adds friction on tiny dev clusters.
- `requests = limits` — guarantees QoS but makes the Job non-burstable; rejected because mikebom benefits from burst CPU.
- Configurable via a NamespaceScan spec field — feature 004+ territory; v0.3 hardcodes safe defaults.

## R5: Job name derivation

**Decision**: `metadata.name = format!("nsscan-{}-{}", sanitized_cr_name, short_image_hash)` where:
- `sanitized_cr_name` is `cr.name()` lowercased + `[^a-z0-9]` replaced with `-` + collapsed runs + truncated to 40 chars.
- `short_image_hash` is the first 7 hex chars of `sha2::Sha256::digest(image_ref.as_bytes())`.
- Final length capped at 63 chars (Kubernetes DNS-1123 limit); truncate `sanitized_cr_name` if needed.

**Rationale**: Deterministic for `(cr_name, image_ref)` (FR-009 satisfied). DNS-1123 compliant (FR-001). Multiple images on the same CR get distinct hashes; the 7-char hash gives a 2^28 collision space, sufficient for the ≤100-image per-CR scale.

**Alternatives considered**:
- Full SHA-256 hex (64 chars) — exceeds 63-char limit; truncating to 7 is conventional (git-style).
- ULID / UUID per Job — loses determinism (FR-009). Rejected.
- Image-ref-as-is sanitized — produces unreadable names for digest-pinned refs (`nginx-sha256-abc123...`). Rejected.

## R6: YAML serialization for the dry-run E2E

**Decision**: Use `serde_yaml::to_string(&job)` where `job: k8s_openapi::api::batch::v1::Job`. The k8s-openapi types derive `Serialize` and produce Kubernetes-conformant YAML when serialized via serde_yaml.

**Rationale**: Already proven by feature 001's `crd_yaml` function (same pattern, same crates). The output is the canonical YAML representation that `kubectl apply -f -` accepts.

**Alternatives considered**:
- `serde_json::to_string` + pipe to `kubectl` (JSON is also accepted) — works but produces less-readable test failure output.
- Hand-rolled YAML emitter — unnecessary; serde_yaml is deterministic for stable input.

## R7: kind dry-run E2E shape

**Decision**: `e2e/tests/scan_job_dryrun.rs` (gated by `MIKEBOM_OPERATOR_E2E=1`) with a single test function:

1. Constructs a fixture `NamespaceScanSpec` in-test.
2. Calls `build_scan_job(&spec, "nginx:1.27.0")` → `Job`.
3. Serializes the Job to YAML.
4. Pipes the YAML through `kubectl apply --dry-run=server -f - --kube-context kind-mikebom-operator-e2e -n default`.
5. Asserts the command succeeds (i.e., the API server validates the manifest).
6. Repeats for `spec.scanFormat = spdx-3-json` to exercise the format branch.
7. Repeats for `spec.mikebomImage = ""` to confirm the builder returns `Err` (the dry-run isn't reached).

**Rationale**: Satisfies constitution VI ("Job-template construction" is explicitly named) at minimal cost — no actual pods are scheduled. The kind cluster is just a real Kubernetes API server validating the manifest. Test runs in seconds and doesn't depend on container-image availability.

**Alternatives considered**:
- Apply the Job for real and watch it run — pulls Chainguard images during E2E (slow, network-dependent), and the placeholder `output-upload` would produce uninteresting output. Rejected for v0.3.
- Use `kubeconform` instead of a real cluster — adds a non-Rust toolchain dependency. Rejected; kind is already a prerequisite for feature 002's E2Es.

## R8: `BuildScanJobError` enum design

**Decision**:

```rust
#[derive(Debug, thiserror::Error)]
pub enum BuildScanJobError {
    #[error("spec.mikebomImage is empty or whitespace-only")]
    EmptyMikebomImage,
    #[error("image_ref is empty or whitespace-only")]
    EmptyImageRef,
}
```

**Rationale**: Two narrow failure modes that the builder catches before constructing a malformed Job. Returning `Result` rather than panicking aligns with FR-012 ("return an error rather than emit a malformed Job"). The variants are user-facing strings — future features can pattern-match on the enum if they need to surface specific errors in status conditions.

**Alternatives considered**:
- Single `BuildScanJobError(String)` newtype — loses pattern-match dispatchability; rejected.
- `anyhow::Error` — fine for binaries but loses type info for the library function's callers; rejected.
- Validate at struct-construction time via a wrapper type — over-engineered for two simple cases; rejected.
