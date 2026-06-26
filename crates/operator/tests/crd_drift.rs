//! CRD drift check: the chart's CRD YAML MUST match what the in-process
//! generator emits from the canonical Rust source. Enforces constitution
//! principle VII (Helm Chart Lockstep) and feature spec FR-005..FR-007.

use operator::crds::namespace_scan::NamespaceScan;
use operator::crds::serialize::crd_yaml;
use pretty_assertions::assert_str_eq;

const CHART_YAML: &str =
    include_str!("../../../charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml");

const REGEN_HINT: &str =
    "\n\nThe checked-in chart CRD YAML drifted from the Rust source. Regenerate with:\n  \
    cargo run --bin mikebom-operator-ctl -- crd \\\n  \
      --output charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml";

#[test]
fn chart_crd_yaml_matches_generator() {
    let generated = crd_yaml::<NamespaceScan>();
    assert_str_eq!(
        CHART_YAML.trim_end_matches('\n'),
        generated.trim_end_matches('\n'),
        "{REGEN_HINT}"
    );
}

#[test]
fn generator_is_deterministic() {
    let a = crd_yaml::<NamespaceScan>();
    let b = crd_yaml::<NamespaceScan>();
    assert_eq!(
        a, b,
        "crd_yaml::<NamespaceScan>() is nondeterministic — investigate before trusting the drift check"
    );
}
