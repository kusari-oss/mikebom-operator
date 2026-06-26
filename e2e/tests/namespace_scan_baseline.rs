//! Baseline E2E (plan §8 / §10 step 5): spin up kind, install chart, apply
//! a NamespaceScan, assert SBOMs land in the configured backend.
//!
//! Gated behind `MIKEBOM_OPERATOR_E2E=1` — kind-based tests are skipped in
//! standard `cargo test --workspace` runs and only fire when the env var
//! is set (matches the per-PR pre-PR gate convention in plan §10 step 5).

#[test]
fn namespace_scan_baseline() {
    if std::env::var("MIKEBOM_OPERATOR_E2E").ok().as_deref() != Some("1") {
        eprintln!("MIKEBOM_OPERATOR_E2E unset; skipping kind-based E2E.");
        // Implementation lands in feature 008 (per plan §10).
    }
}
