# Quickstart: scan-Job builder

## For feature 004 contributors wiring the reconciler

After this feature lands:

```rust
use operator::crds::namespace_scan::NamespaceScan;
use operator::scan_job::build_scan_job;

let scan = /* get the NamespaceScan you're reconciling */;
let image_ref = "nginx:1.27.0"; // typically from pod enumeration in feature 007+
let cr_name = scan.metadata.name.as_deref().unwrap_or("unknown");

let job = build_scan_job(&scan.spec, cr_name, image_ref)?;

// Set the OwnerReference so deleting the NamespaceScan cleans up the Job:
job.metadata.owner_references = Some(vec![/* OwnerReference pointing at scan */]);

// Apply to the cluster:
let api: Api<Job> = Api::namespaced(client.clone(), &operator_namespace);
api.create(&PostParams::default(), &job).await?;
```

The builder is pure: same inputs → same output. No I/O, no clock, no environment reads. Feature 004's reconciler can call it freely in a loop without worrying about side effects.

## For reviewers checking the Job shape

```sh
# Run the unit tests — they assert every FR (FR-001..FR-012) is satisfied.
cargo test --workspace --test scan_job

# Read the test names — each maps to an FR per data-model.md §5.
cargo test --workspace --lib operator::scan_job:: -- --list
```

## For local dry-run validation (kind cluster needed)

```sh
kind create cluster --config e2e/kind-cluster.yaml
MIKEBOM_OPERATOR_E2E=1 cargo test --test scan_job_dryrun
```

The test serializes the builder's output to YAML and pipes it through
`kubectl apply --dry-run=server` against the kind cluster. No pod is
actually scheduled — only the API server's manifest validation runs.

## For debugging a Job's shape interactively

```sh
# A small helper binary you can write locally (not shipped):
cargo run --bin mikebom-operator-ctl -- --help

# Or, inline in a Rust unit test:
cargo test --workspace --lib operator::scan_job::tests:: -- --nocapture
# Tests with `eprintln!(serde_yaml::to_string(&job).unwrap())` will dump the Job to stderr.
```

## Common Q&A

| Question | Answer |
|----------|--------|
| Where does `output-upload`'s real logic land? | Feature 004 (PVC), 005 (S3), or 006 (OCI). The builder's `output-upload` container is a debug placeholder in v0.3. |
| Can I override the init-pull image? | Not via `NamespaceScanSpec` yet. Edit `crates/operator/src/scan_job/mod.rs` constant and regen the digest. v0.4+ may expose it. |
| What if `spec.mikebomImage` is unset? | The builder returns `Err(BuildScanJobError::EmptyMikebomImage)`. Feature 002's reconciler already validates this at admission, so the error path is defense-in-depth. |
| How do I add a NEW container to the Job? | Don't, in v0.3. Adding one breaks FR-002's "exactly three" contract; bump the feature spec first. |
| The Job's name uses only 7 chars of hash — won't there be collisions? | At ≤100 images per CR, the birthday-problem collision rate is ≈ 4×10⁻⁶. Acceptable for v0.3; expand if we ever see one in production. |

## Performance expectations

| Operation | Budget |
|-----------|--------|
| `build_scan_job(spec, cr_name, image_ref)` | < 100µs (microseconds) — pure SHA-256 + struct construction |
| Full unit-test suite | < 1s (SC-001) |
| Dry-run E2E (one fixture call + `kubectl --dry-run=server`) | < 5s (network round-trip to kind API server) |
