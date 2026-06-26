//! Deterministic YAML serializer for `kube::CustomResource`-derived types.
//!
//! Single source of truth for both the `mikebom-operator-ctl crd` subcommand
//! and the `crd_drift.rs` integration test — see plan §"Structure Decision"
//! and constitution principle VII.

use kube::CustomResourceExt;

/// Serialize the CRD definition for `K` as a YAML string.
///
/// Output is deterministic: kube's `crd()` returns a `CustomResourceDefinition`
/// whose serde derives use stable Rust struct-field order, and `serde_yaml::to_string`
/// is deterministic for any input with stable iteration order. Determinism is
/// enforced by the `generator_is_deterministic` test in `tests/crd_drift.rs`.
pub fn crd_yaml<K: CustomResourceExt>() -> String {
    serde_yaml::to_string(&K::crd()).expect("CustomResourceDefinition serialization is infallible")
}
