# Feature Specification: CRD YAML generator + drift check

**Feature Branch**: `001-crd-yaml-generator`

**Created**: 2026-06-25

**Status**: Draft

**Input**: User description: "Wire mikebom-operator-ctl crd subcommand to emit NamespaceScan CRD YAML from the kube::CustomResource derive in crates/operator/src/crds/namespace_scan.rs, and add a CI check that the chart's charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml matches the generator output (constitution VII)"

## Clarifications

### Session 2026-06-25

- Q: Should the CI drift check be implemented as a `cargo test` integration test, or as a dedicated CI workflow step that shells out to the binary? → A: cargo test integration test in the operator crate (Option A) — verification lives next to the code, runs locally via `cargo test --workspace`, no shell layer.
- Q: For v0.1 with exactly one CRD, does the `crd` subcommand take a positional CRD name, or no argument with hardcoded `NamespaceScan`? → A: No positional argument in v0.1 (Option A) — hardcoded to `NamespaceScan`. Future CRDs in v0.2+ slot in via an optional positional argument (default `NamespaceScan`) or a sibling subcommand — non-breaking.

## User Scenarios & Testing *(mandatory)*

### User Story 1 — Regenerate CRD YAML from Rust source (Priority: P1)

A contributor who has just edited the `NamespaceScan` Rust struct can run a single command to regenerate the chart's CRD YAML so it matches the new struct. They never hand-edit YAML to mirror Rust changes.

**Why this priority**: Without the regen command, the chart YAML and Rust struct drift on every CRD change. Contributors either skip the YAML update (silently breaking the chart for end users) or duplicate effort by editing both. P1 because every later milestone that touches the CRD assumes this command exists.

**Independent Test**: Modify any field in the `NamespaceScan` struct, run the regen command, diff the output against the prior chart YAML, observe the change reflected in the output. Delivers value standalone — even without the CI half (P2), contributors can use it manually.

**Acceptance Scenarios**:

1. **Given** the `NamespaceScan` Rust struct is the canonical source, **When** a contributor runs the regen command, **Then** the chart's CRD YAML is written to the configured destination and reflects every field, type, and required/optional marker from the Rust source.
2. **Given** the contributor runs the regen command against an unchanged struct twice, **When** the outputs are compared byte-for-byte, **Then** they are identical (deterministic output).

---

### User Story 2 — CI rejects drifted PRs (Priority: P2)

A reviewer of an incoming PR can trust that if the PR changes the `NamespaceScan` struct, the chart's CRD YAML in the same PR matches. CI fails the PR if the two diverge.

**Why this priority**: P1 makes the regen possible; P2 makes it mandatory. Constitution principle VII requires CI verification that the checked-in chart YAML matches the generator output. Without P2, P1 is purely advisory and drift can land in main.

**Independent Test**: Open a PR that modifies the Rust struct without regenerating the chart YAML. Observe CI fails with a diagnostic that names the regen command and shows the offending diff.

**Acceptance Scenarios**:

1. **Given** a PR has a struct change but no matching YAML update, **When** CI runs, **Then** the build fails with a diagnostic naming the regen command and showing the diff.
2. **Given** a PR has matched struct + YAML changes, **When** CI runs, **Then** the drift check passes.
3. **Given** a PR touches neither the struct nor the chart YAML, **When** CI runs, **Then** the drift check passes (no false positives).

---

### User Story 3 — Replace the chart placeholder (Priority: P3)

The current `charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml` is a placeholder comment, not a valid CRD. The first commit on this feature replaces it with real generated output so the Helm chart actually installs the CRD.

**Why this priority**: Strictly speaking, this is a one-time effect of P1 and P2 landing. Calling it out separately because the chart as-shipped today wouldn't install the CRD — a chart-consumer reading the repo would be surprised.

**Independent Test**: After this feature lands, `helm install charts/mikebom-operator/` succeeds in a fresh kind cluster and `kubectl get crd namespacescans.kusari.dev` returns a valid CRD.

**Acceptance Scenarios**:

1. **Given** the chart YAML is the generator output (not the placeholder), **When** the chart is installed in a kind cluster, **Then** the `NamespaceScan` CRD is registered and `kubectl apply -f examples/namespacescan.yaml` is accepted by the API server.

---

### Edge Cases

- The generator command run with no arguments writes to stdout so it composes with shell redirection. An explicit `--output <path>` writes in-place for chart updates.
- If the operator crate doesn't compile, the generator can't build — failure surfaces as a compile error before the drift check runs.
- The generated YAML must be byte-identical across repeated runs of the same struct; otherwise the drift check would flap. The generator must produce stable field order and no embedded timestamps or random IDs.
- For v0.1 there is exactly one CRD. A future second CRD would extend the subcommand syntax; the v0.1 contract MUST NOT preclude that extension.
- If a PR intentionally lands struct + YAML changes in two separate commits but in the same PR, the drift check uses the final tree state and passes (single-PR scope, not per-commit).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The `mikebom-operator-ctl` binary MUST expose a `crd` subcommand that emits the `NamespaceScan` CRD as a Kubernetes-compatible YAML manifest (`apiextensions.k8s.io/v1` `CustomResourceDefinition`).
- **FR-002**: The emitted YAML MUST be derived programmatically from `crates/operator/src/crds/namespace_scan.rs` — the same source the reconciler consumes — with no hand-written YAML intermediate.
- **FR-003**: The `crd` subcommand MUST write to standard output by default; supplying `--output <path>` MUST write to that path instead (overwriting an existing file).
- **FR-004**: Successive invocations of the `crd` subcommand against an unchanged Rust source MUST produce byte-identical output (deterministic key order, no embedded timestamps, no nondeterministic IDs).
- **FR-005**: CI MUST run the generator and compare its output to `charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml` on every PR; a non-empty diff MUST fail the build.
- **FR-006**: When the CI drift check fails, the failure message MUST name the exact command a contributor should run locally to fix the drift.
- **FR-007**: The CI drift check MUST be implemented as a Rust integration test (e.g., `crates/operator/tests/crd_drift.rs`) that runs as part of `cargo test --workspace` — not as a separate CI workflow shell step. The test MUST NOT require any toolchain beyond what the workspace already builds against (no `kubectl`, no `kopium`, no external schema validator).
- **FR-008**: The `mikebom-operator-ctl crd` subcommand MUST take no positional argument in v0.1; the emitted CRD is hardcoded to `NamespaceScan`. The implementation MUST be structured so a future CRD (v0.2+) can be added as a non-breaking change — e.g., by introducing an optional positional argument defaulting to `NamespaceScan`, or by adding a sibling subcommand.
- **FR-009**: The first PR that ships this feature MUST replace the placeholder content in `charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml` with the real generated CRD manifest.

### Key Entities

- **NamespaceScan Rust struct**: Canonical source-of-truth at `crates/operator/src/crds/namespace_scan.rs`, annotated with `#[derive(CustomResource, JsonSchema, ...)]`.
- **Chart CRD manifest**: Generated artifact at `charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml`. Consumed by Helm chart installs; the only CRD definition the cluster sees.
- **Drift check**: CI assertion that the Rust struct and the chart manifest are in sync. Pass/fail with diagnostic output.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A contributor who modifies the `NamespaceScan` struct can regenerate the chart YAML and verify alignment in under 30 seconds (single command + verification).
- **SC-002**: 100% of PRs that change `crates/operator/src/crds/*.rs` without a matching `charts/mikebom-operator/crds/*.yaml` update are caught by CI before merge.
- **SC-003**: Across the first 20 unrelated PRs (touching neither file), zero false-positive drift failures occur.
- **SC-004**: After this feature lands, installing the chart into a fresh kind cluster registers the `NamespaceScan` CRD and the example CR (`examples/namespacescan.yaml`) is accepted by the API server.
- **SC-005**: A contributor new to the operator can find the regen command in `docs/crd-reference.md` and run it successfully without prior knowledge of `kube::CustomResource` internals.

## Assumptions

- The `NamespaceScan` Rust struct (and any future CRD struct) carries `#[derive(CustomResource, JsonSchema)]` — the `JsonSchema` derive from `schemars` is the bridge that lets `kube::CustomResourceExt::crd()` produce a schema. The bootstrap workspace deps already wire this.
- CI runs on Linux with the same Rust toolchain version as the workspace. No cross-platform output normalization (line endings, etc.) is required because all checked-in files are LF.
- For v0.1 there is exactly one CRD (`NamespaceScan`). The "future second CRD" scenario is acknowledged in FR-008 but not implemented here.
- The chart's existing CRD YAML file is currently a placeholder comment, not a valid CRD. Replacing it with real generated output is part of this feature's first commit (FR-009).
- Both chart consumers and the operator's reconciler agree on field semantics because both derive from the same Rust source. This is constitution principle VII expressed as a single-source-of-truth contract.
