# Quickstart: CRD YAML generator + drift check

## For contributors editing `NamespaceScan`

After modifying `crates/operator/src/crds/namespace_scan.rs`:

```sh
cargo run --bin mikebom-operator-ctl -- crd \
  --output charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml
cargo test --workspace
```

The `cargo test` step exercises `crates/operator/tests/crd_drift.rs`, which asserts the chart YAML matches the in-process generator. If it passes locally, CI will pass too — both run the same test.

## For reviewers seeing a CI drift failure

The CI failure message names the regen command verbatim. Reviewers should:

1. Confirm the PR intentionally changes the CRD shape (vs. an accidental struct edit).
2. Ask the author to run the regen command and push the result, OR revert the struct change if it was unintentional.

The drift check never auto-fixes — that would let surprising changes land silently.

## For chart consumers

After this feature lands, the chart ships a real `CustomResourceDefinition`. Install + verify:

```sh
helm install mikebom-operator charts/mikebom-operator \
  -n kusari-operator --create-namespace
kubectl get crd namespacescans.kusari.dev
kubectl apply -f examples/namespacescan.yaml
```

(`examples/namespacescan.yaml` was scaffolded by the bootstrap PR and remains a no-op until feature 002's reconciler lands.)

## CI behavior

- `.github/workflows/ci.yml` already runs `cargo test --workspace`. The drift check is a standard integration test in `crates/operator/tests/crd_drift.rs`; no new CI workflow steps, no new toolchain.
- Kind-based E2E for CRD install lives in `e2e/tests/crd_install.rs`, gated behind `MIKEBOM_OPERATOR_E2E=1`. CI runs E2Es per constitution VI.

## Troubleshooting

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| `crd_drift.rs` fails locally, no struct change | stale chart YAML (perhaps a force-push or revert clobbered a previous regen) | Run the regen command; commit the result. |
| `generator_is_deterministic` test fails | new kube-rs / serde_yaml version introduced map-order nondeterminism | Investigate before pinning around it — silent flapping is the failure mode this test catches. |
| `cargo run --bin mikebom-operator-ctl -- crd` panics | `NamespaceScan::crd()` derivation broke (likely from a struct edit that confuses `schemars`) | Read the panic message; usually a `#[serde(...)]` attribute mismatch with the `JsonSchema` derive. |
