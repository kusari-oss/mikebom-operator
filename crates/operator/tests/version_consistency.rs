//! Feature 010: version consistency invariant.
//!
//! Asserts the three in-repo version strings agree:
//!   - workspace `Cargo.toml` `[workspace.package] version`
//!   - `charts/mikebom-operator/Chart.yaml` `version`
//!   - `charts/mikebom-operator/Chart.yaml` `appVersion`
//!
//! Runs on every `cargo test --workspace` invocation. Catches drift on PRs,
//! well before a release attempt.
//!
//! At release time, `.github/scripts/check-versions.sh` additionally asserts
//! all three match the git tag (minus the `v` prefix).

use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR points at the operator crate; walk up two levels:
    // crates/operator → crates → repo root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("two parent directories above CARGO_MANIFEST_DIR (repo root)")
        .to_path_buf()
}

fn read_cargo_workspace_version(cargo_toml: &Path) -> String {
    let text = fs::read_to_string(cargo_toml).expect("read Cargo.toml");
    // Look for the `[workspace.package]` table, then the first `version = "..."` line.
    let mut in_workspace_package = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_workspace_package = trimmed == "[workspace.package]";
            continue;
        }
        if !in_workspace_package {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("version") {
            // form: version = "X.Y.Z"
            let rest = rest.trim_start();
            let rest = rest.strip_prefix('=').expect("version line has '='");
            let rest = rest.trim();
            let rest = rest
                .strip_prefix('"')
                .expect("version value starts with \"");
            let end = rest.find('"').expect("version value ends with \"");
            return rest[..end].to_string();
        }
    }
    panic!("could not find `[workspace.package] version` in {cargo_toml:?}");
}

fn read_chart_versions(chart_yaml: &Path) -> (String, String) {
    let text = fs::read_to_string(chart_yaml).expect("read Chart.yaml");
    let parsed: serde_yaml::Value = serde_yaml::from_str(&text).expect("parse Chart.yaml");
    let map = parsed
        .as_mapping()
        .expect("Chart.yaml top-level is a mapping");
    let version = map
        .get(serde_yaml::Value::String("version".to_string()))
        .expect("Chart.yaml has `version` key")
        .as_str()
        .expect("Chart.yaml `version` is a string")
        .to_string();
    let app_version = map
        .get(serde_yaml::Value::String("appVersion".to_string()))
        .expect("Chart.yaml has `appVersion` key")
        .as_str()
        .expect("Chart.yaml `appVersion` is a string")
        .to_string();
    (version, app_version)
}

#[test]
fn cargo_workspace_version_matches_chart_version_and_appversion() {
    let root = workspace_root();
    let cargo_version = read_cargo_workspace_version(&root.join("Cargo.toml"));
    let chart_path = root.join("charts/mikebom-operator/Chart.yaml");
    let (chart_version, chart_app_version) = read_chart_versions(&chart_path);

    assert_eq!(
        cargo_version, chart_version,
        "Cargo.toml workspace version ({cargo_version}) MUST match Chart.yaml \
         version ({chart_version}). Update both atomically — see \
         specs/010-release-pipeline/spec.md FR-006.",
    );
    assert_eq!(
        cargo_version, chart_app_version,
        "Cargo.toml workspace version ({cargo_version}) MUST match Chart.yaml \
         appVersion ({chart_app_version}). Update both atomically — see \
         specs/010-release-pipeline/spec.md FR-006.",
    );
}
