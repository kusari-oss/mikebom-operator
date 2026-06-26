# mikebom-operator Constitution

These principles govern the `kusari-oss/mikebom-operator` repo. They are
**operator-specific** and intentionally diverge from
`kusari-oss/mikebom`'s constitution — the operator's coupling to mikebom
is the opaque-JSON-consumer model (see Principle II), not a shared Rust
type system, so mikebom's "Pure Rust Zero C", "eBPF-Only Observation", and
"Three-Crate Architecture" principles do not apply here.

## Core Principles

### I. Pure Rust where reasonable

Operator code is Rust. C transitives reaching us via `kube-rs`, `rustls`,
`aws-sdk-s3`, etc. are accepted as ecosystem reality — we don't pin a
"zero C" goal we couldn't honor. New direct dependencies that introduce C
linkage MUST be justified in their PR description.

### II. USE not EMBED (NON-NEGOTIABLE)

The operator orchestrates ephemeral `batch/v1` Job pods that run the
published `ghcr.io/kusari-oss/mikebom:<tag>` container image. The
operator MUST NOT statically link mikebom-cli code, MUST NOT pull in
`mikebom-common` as a Rust dependency, and MUST treat all SBOM artifacts
as opaque JSON blobs. This is the architectural premise of the entire
repo (see plan `sparkling-chasing-bee.md` §2 Decision) — drifting from
it requires a constitution amendment, not a PR.

### III. Fail Closed on RBAC

When a `NamespaceScan` (or future CR) requests narrower scope than the
operator's installed RBAC grants, the operator MUST honor the narrower
scope. The operator MUST NEVER silently fall back to broader RBAC or
broader namespace selection when the requested scope is unsatisfiable —
it MUST surface a `Ready=False` condition with a `RBACInsufficient`
reason and stop, not scan more than asked.

### IV. CRD Backward Compatibility

Once a `v1alpha1` field is documented in `docs/crd-reference.md` or
shipped in a tagged release, breaking renames, type changes, or
semantic shifts require a `v1alpha2` migration with a conversion
webhook. Silent shape drift inside `v1alpha1` is prohibited. Adding
new optional fields to `v1alpha1` is allowed.

### V. SBOM-Format Agnostic

The operator MUST accept CDX 1.6, SPDX 2.3, and SPDX 3.x outputs from
mikebom and treat them as opaque blobs for storage and upload. The
operator MUST NOT parse mikebom-internal annotations, MUST NOT
interpret SBOM contents to drive control flow, and MUST NOT re-serialize
SBOMs (pass-through only). This is the concrete enforcement of
Principle II at the data layer.

### VI. Hermetic E2E Tests (NON-NEGOTIABLE)

Every PR that touches reconciler logic, Job-template construction, CRD
shape, or RBAC MUST exercise a `kind`-based E2E that:
1. spins up a kind cluster from `e2e/kind-cluster.yaml`,
2. installs the operator via the Helm chart in `charts/mikebom-operator/`,
3. applies a `NamespaceScan` CR targeting a fixture namespace,
4. asserts `.status.conditions[Ready]=True` AND that SBOM artifacts
   land in the configured backend within a bounded timeout.

E2Es run behind `MIKEBOM_OPERATOR_E2E=1` locally; CI runs them
unconditionally. No PR merges without green E2E.

### VII. Helm Chart Lockstep

Every tagged operator image release ships with a matching Helm chart
version. The chart's CRD schemas (`charts/mikebom-operator/crds/`) are
**generated** from the Rust `kube::CustomResource` derives in
`crates/operator/src/crds/` via `mikebom-operator-ctl crd`, never
hand-edited. CI MUST verify that the checked-in CRD YAML matches the
output of the generator for every PR.

## Additional Constraints

- **Stack pins**: `kube = "0.97"`, `k8s-openapi = "0.23"` (feature
  `v1_31`), `tokio = "1"` (`rt-multi-thread`). Bumps to either of the
  Kubernetes-client crates require a PR that runs the full E2E suite
  against the bumped versions.
- **Container base**: distroless (`gcr.io/distroless/cc-debian12:nonroot`)
  for the operator image. No shell, no package manager in the production
  image.
- **Leader election**: required for `replicas > 1`. Implemented via
  `kube::runtime::reflector::Lease` against the `coordination.k8s.io`
  API. Operators MUST NOT race on `NamespaceScan` reconcile.
- **No multi-cluster (v0.1)**: scope is single-cluster. Multi-cluster
  via Flux/ArgoCD mirroring is out-of-scope for v0.x.

## Development Workflow

- **Per-PR pre-PR gate** mirrors mikebom's `./scripts/pre-pr.sh`:
  - `cargo +stable fmt --all -- --check`
  - `cargo +stable clippy --workspace --all-targets -- -D warnings`
  - `cargo +stable test --workspace`
  - `helm lint charts/mikebom-operator/`
  - `MIKEBOM_OPERATOR_E2E=1 cargo test --test namespace_scan_baseline`
    (locally, requires kind; CI handles it unconditionally).
- **Speckit pipeline** for every operator feature:
  `/speckit-specify → /speckit-clarify → /speckit-plan → /speckit-tasks
  → /speckit-analyze → /speckit-implement`. Branch numbering is
  per-repo sequential starting at `001-…`.
- **Conventional commits** with the feature-branch number in the body
  for traceability (e.g., `feat(operator): NamespaceScan reconciler
  skeleton (002)`).

## Governance

This Constitution supersedes any informal practice or contributor
preference. Amendments require:
1. A PR titled `constitution: amend Principle N` (or `add Principle N`)
   that updates this file AND every place the principle is enforced
   (docs, CI, code comments).
2. Maintainer approval from at least one Kusari OSS maintainer.
3. A migration plan if the amendment changes a binding constraint
   (e.g., bumping kube-rs major version, dropping a scan format).

All PRs and reviews MUST verify compliance with these principles.
Complexity that conflicts with a principle MUST be justified in the
PR description or rejected.

**Version**: 1.0.0 | **Ratified**: 2026-06-25 | **Last Amended**: 2026-06-25
