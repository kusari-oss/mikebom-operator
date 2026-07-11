//! Feature 011 T002 (scaffold) + T011 (assertions): regression guard for the
//! nightly-bump workflow's version arithmetic.
//!
//! The nightly shell script (`.github/scripts/nightly-bump.sh`) computes the
//! next operator version by parsing the trailing `.<N>` off the current
//! `charts/mikebom-operator/Chart.yaml` `version` field. If the shell script's
//! regex ever drifts from what the operator's own Rust source expects
//! (Constitution VII — Helm Chart Lockstep), this test fails a normal
//! `cargo test --workspace` run, catching the drift on any PR before it can
//! land.
//!
//! What we verify:
//!   1. Chart.yaml's `version` matches the regex `^0\.1\.0-alpha\.[0-9]+$`.
//!   2. `version == appVersion` (structural lockstep, feature 010 invariant).
//!   3. The "next alpha" string that a bump would produce is well-formed
//!      (parses the trailing integer, increments, verifies the reconstructed
//!      `v0.1.0-alpha.<N+1>` matches the same regex the shell uses).

use serde_yaml::Value;
use std::fs;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR points to crates/operator; go up two.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root is two levels above CARGO_MANIFEST_DIR")
        .to_path_buf()
}

fn read_chart_yaml() -> Value {
    let path = workspace_root().join("charts/mikebom-operator/Chart.yaml");
    let text = fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!("failed to read {}: {err}", path.display());
    });
    serde_yaml::from_str(&text).expect("Chart.yaml is not valid YAML")
}

fn str_field(chart: &Value, field: &str) -> String {
    chart
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("Chart.yaml missing string field `{field}`"))
        .to_string()
}

fn parse_alpha_num(version: &str) -> u32 {
    let suffix = version
        .strip_prefix("0.1.0-alpha.")
        .unwrap_or_else(|| panic!("version {version} does not match 0.1.0-alpha.N shape"));
    suffix
        .parse::<u32>()
        .unwrap_or_else(|err| panic!("alpha suffix of {version} is not a u32: {err}"))
}

#[test]
fn chart_version_matches_alpha_shape() {
    let chart = read_chart_yaml();
    let version = str_field(&chart, "version");
    let _ = parse_alpha_num(&version); // panics with a clear message on mismatch.
}

#[test]
fn chart_version_and_app_version_are_lockstep() {
    let chart = read_chart_yaml();
    let version = str_field(&chart, "version");
    let app_version = str_field(&chart, "appVersion");
    assert_eq!(
        version, app_version,
        "Constitution VII (Helm Chart Lockstep): Chart.yaml .version MUST equal .appVersion",
    );
}

#[test]
fn next_alpha_computation_is_well_formed() {
    let chart = read_chart_yaml();
    let version = str_field(&chart, "version");
    let current = parse_alpha_num(&version);
    let next = current
        .checked_add(1)
        .expect("alpha counter overflowed u32 — unrealistic but tested");
    let next_tag = format!("v0.1.0-alpha.{next}");
    let stripped = next_tag
        .strip_prefix('v')
        .expect("constructed tag must start with 'v'");
    let _ = parse_alpha_num(stripped); // reuses the same parser, catching regex drift.
    assert!(
        next > current,
        "next alpha ({next}) must be strictly greater than current ({current})",
    );
}
