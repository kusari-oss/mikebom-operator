# Implementation Plan: Nightly mikebom rebuild

**Branch**: `011-nightly-mikebom-rebuild` | **Date**: 2026-07-10 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/011-nightly-mikebom-rebuild/spec.md`

## Summary

Two new GitHub Actions workflows plus a small library of POSIX shell scripts
turn the operator into a mikebom-follower. The **`nightly-mikebom-bump.yml`**
workflow runs on a nightly cron (and on `workflow_dispatch`): it reads the
currently pinned mikebom tag from `charts/mikebom-operator/values.yaml`, queries
the highest available `v0.1.0-alpha.N` release on `kusari-oss/mikebom`,
verifies the multi-arch image manifest exists on `ghcr.io/kusari-oss/mikebom`,
consults recent PR history to derive the known-bad set (per FR-017), then either
no-ops with a clear run summary or opens a bump PR containing (a) the file-wide
tag replacement (same 12-file surface as the manual `v0.1.0-alpha.57` bump), (b)
the regenerated CRD YAML (per Constitution VII), and (c) the operator's own
version bump in structural lockstep (Cargo.toml + Chart.yaml `version` +
`appVersion`). It enables GitHub's native auto-merge on the PR, gated by the
existing `ci.yml` checks + Kusari Inspector. The second workflow,
**`tag-on-nightly-merge.yml`**, fires on `push` to `main`, detects a merged
bump commit by author + commit-trailer signature, computes the operator tag
from Chart.yaml, and pushes the corresponding `vX.Y.Z-alpha.N` tag — which
triggers the existing `release.yml` to build, sign, attest, and publish the
release. Failure signaling (FR-012, FR-016) uses the standard `gh issue create`
+ workflow annotations, with de-dup keyed on `(failing_step, error_class)`.
Stale-PR handling (FR-018) is a pre-run `gh pr list` check that skips the run
if an unresolved prior bump PR exists.

## Technical Context

**Language/Version**: N/A for compiled code. Implementation is
GitHub Actions YAML + POSIX `/bin/sh` (compatible with the same interpreter the
existing `.github/scripts/check-versions.sh` targets) + `gh`, `git`, `yq`, and
`docker` CLIs available on `ubuntu-latest` runners.

**Primary Dependencies**:
- Existing repo tooling (SHA-pinned per Kusari Inspector rules):
  - `actions/checkout` — already used and pinned in `release.yml` /
    `ci.yml`; reused verbatim.
  - `mikefarah/yq` — already pinned in `release.yml`; reused for reading
    `values.yaml` and `Chart.yaml` fields.
- No new third-party actions. All PR opening + labeling + auto-merge
  enablement uses the preinstalled `gh` CLI + the workflow's own
  `GITHUB_TOKEN` (or an elevated bot identity — see Research §R1). This
  keeps the SHA-pinned action count at zero-delta.
- The mikebom release lookup uses `gh api repos/kusari-oss/mikebom/releases`
  (no extra action required, same auth as everything else).
- Multi-arch manifest verification uses `docker manifest inspect` (bundled
  on `ubuntu-latest`; no auth needed for the public `ghcr.io/kusari-oss/mikebom`
  namespace).

**Storage**: N/A — no long-lived state file. The "known-bad" set (FR-017)
is *derived* from PR history via `gh pr list` (see Research §R3). This choice
is a deliberate simplification over an in-repo state file.

**Testing**:
- **Shellcheck** on every new `.github/scripts/nightly-*.sh` script (adds a
  step to `ci.yml`, same pattern as `check-versions.sh`).
- **Unit test for the version-bump function**: a small Rust test in
  `crates/operator/tests/nightly_version_bump.rs` that (a) reads
  `Chart.yaml.version`, (b) computes the "next alpha" candidate the same way
  the bump script does, (c) asserts monotonic increment. Runs in every
  `cargo test --workspace` — catches drift in the shell script's version
  arithmetic against the Rust source of truth (Constitution VII, generation
  side).
- **Workflow dry-run**: `workflow_dispatch` with a `dry_run: true` input on
  the nightly workflow — runs detection + bump script + git ops in a scratch
  branch but does NOT push, open PR, or leave any state on the remote. Same
  pattern as `release.yml`'s dry-run rehearsal. Documented in the runbook.
- **Post-first-successful-run smoke**: a manual maintainer checklist —
  inspect the auto-opened PR's file diff (should match the 12-file live
  surface plus the CRD regeneration + version files), inspect the merged
  commit, verify the auto-pushed tag matches Chart.yaml, verify the release
  page appears with signed artifacts. Added to `docs/release-runbook.md`.

**Target Platform**: GitHub-hosted `ubuntu-latest` runners. No self-hosted
runners needed. The workflow is repo-scoped (does not cross into
`kusari-oss/mikebom`).

**Performance Goals**:
- No-op nightly run (SC-002): under 5 minutes end-to-end. Dominated by
  runner cold-start + a single mikebom API call + a `docker manifest inspect`.
- Full bump nightly run (SC-003): the detection + bump + PR open is under 3
  minutes on the nightly workflow itself; the follow-on CI + auto-merge +
  tag push + `release.yml` chain is bounded by the release pipeline's
  existing ≤15-min budget (feature 010's SC-001). Total end-to-end under
  the spec's 45-minute budget with margin.

**Constraints**:
- **Constitution VII (Helm Chart Lockstep)**: the CRD YAML in
  `charts/mikebom-operator/crds/` is generated from the Rust struct, not
  hand-edited. The nightly bump script MUST regenerate the CRD after
  updating the Rust doc-comment, matching the existing CI verification.
- **Memory: pin third-party deps by default**: no new GH Actions are added
  by this feature (only shell scripts + existing SHA-pinned actions). No
  new SHA to manage.
- **Feature 010's version-consistency guarantee**: the bump script MUST
  update Cargo.toml + Chart.yaml `version` + Chart.yaml `appVersion` in
  the same commit; otherwise `release.yml`'s `versions` job will reject
  the tag push. This is enforced by the existing check, but the plan calls
  it out.
- **No long-lived secrets**: PR opening + auto-merge uses `GITHUB_TOKEN`.
  If that token can't enable auto-merge on protected branches (repo
  setting), the fallback is a dedicated GitHub App or a fine-grained PAT
  in a repo secret. Decision deferred to Research §R1.
- **Concurrency**: a `concurrency:` block on both nightly workflows
  prevents overlapping runs. `release.yml`'s own execution is not blocked
  by nightly workflows — but the nightly's PR-check step (FR-018) prevents
  a fresh PR from being opened while a prior one is unresolved, which
  transitively prevents the tag-push workflow from firing.

**Scale/Scope**: one detection run per 24h + occasional `workflow_dispatch`.
Mikebom's alpha cadence has been ~1 per 2-3 days in the observed sample, so
in steady state ~30-40% of nightly runs will actually open a PR.

## Constitution Check

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Pure Rust where reasonable | PASS / N/A | Feature is workflow YAML + shell. Adds one small Rust integration test in `crates/operator/tests/nightly_version_bump.rs` — uses `std::fs` + existing `serde_yaml` dep, no new C linkage. |
| II. USE not EMBED (NON-NEGOTIABLE) | PASS / N/A | Feature is release infra. No mikebom source code is embedded; the pinned image tag is a string, treated the same as feature 010's chain. |
| III. Fail Closed on RBAC | PASS / N/A | No runtime RBAC. GitHub permissions follow least-privilege: nightly workflow gets `contents: write` (bump commit) + `pull-requests: write` (open PR) + `issues: write` (failure signal); tag-push workflow gets `contents: write` only. No `id-token: write` (nightly workflows don't call cosign — `release.yml` retains that role). |
| IV. CRD Backward Compatibility | PASS | The bump doesn't touch CRD field shapes — only the pinned mikebom tag string in the CRD's doc-comment description (which surfaces as a description in the generated OpenAPI schema). No `v1alpha1`→`v1alpha2` implications. |
| V. SBOM-Format Agnostic | PASS / N/A | Nightly doesn't parse SBOMs. Downstream `release.yml` continues to produce CycloneDX SBOM attestations on the released image. |
| VI. Hermetic E2E Tests (NON-NEGOTIABLE) | PASS | The bump PR only changes the mikebom image string, does NOT touch reconciler logic, Job templates, CRD shape, or RBAC. The E2Es in `ci.yml` run on the PR anyway (Constitution VI enforcement point) — the nightly workflow does not need its own E2E. If E2Es fail on the auto-opened PR, auto-merge does not fire and the PR is marked known-bad on the next run (FR-017). This is the intended safety net. |
| VII. Helm Chart Lockstep | PASS | The bump script (a) updates Cargo.toml + Chart.yaml versions in structural lockstep (FR-008); (b) regenerates the CRD YAML via `cargo run --bin mikebom-operator-ctl -- crd > charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml`, matching feature 001's generation invariant; (c) `release.yml`'s existing `versions` pre-flight rejects any residual drift before signing. |

All gates pass. No `## Complexity Tracking` section needed.

## Project Structure

### Documentation (this feature)

```text
specs/011-nightly-mikebom-rebuild/
├── plan.md                              # this file
├── spec.md                              # spec (3 in-session clarifications, all resolved)
├── checklists/requirements.md           # 15/15 passing
├── research.md                          # Phase 0: 7 decisions (bot identity, auto-merge mechanism, known-bad model, post-merge tag trigger, manifest verify, latest-alpha lookup, concurrency lock)
├── data-model.md                        # Phase 1: workflow graph + entities (Mikebom Alpha, Operator Alpha, Bump PR, Nightly Run) + FR→artifact mapping
├── quickstart.md                        # Phase 1: maintainer's operational guide — dry-run rehearsal, first-run inspection checklist, override/hold procedures
├── contracts/
│   ├── nightly-workflow.md              # Contract: nightly-mikebom-bump.yml inputs/outputs + step ordering + failure semantics + PR schema
│   └── tag-workflow.md                  # Contract: tag-on-nightly-merge.yml inputs/outputs + commit-trailer signature + tag-push safety
└── tasks.md                             # /speckit-tasks output (not created here)
```

### Source / config files modified or added

```text
.github/
├── workflows/
│   ├── nightly-mikebom-bump.yml         # NEW: schedule + workflow_dispatch. Runs detection, bump, PR open. Enables auto-merge.
│   └── tag-on-nightly-merge.yml         # NEW: push-triggered on main. Detects merged nightly bump commit (author + commit-trailer). Pushes matching operator tag → triggers release.yml.
└── scripts/
    ├── nightly-detect.sh                # NEW: POSIX shell. Reads current pin from values.yaml, queries mikebom releases, filters to alpha, semver-sorts, verifies manifest, checks known-bad set. Writes decision outputs to $GITHUB_OUTPUT.
    ├── nightly-bump.sh                  # NEW: performs the file-wide tag replacement (grep-based discovery + sed), regenerates CRD via cargo, bumps operator version in Cargo.toml + Chart.yaml.
    ├── nightly-open-pr.sh               # NEW: git branch + commit + push + gh pr create + gh pr merge --auto. Uses commit trailer `Nightly-Bump-Target: v0.1.0-alpha.N` for the tag workflow to detect.
    └── nightly-tag.sh                   # NEW: reads Chart.yaml appVersion, pushes matching tag. Called by tag-on-nightly-merge.yml.

charts/
└── mikebom-operator/                    # UNCHANGED structure. Files touched *by the nightly at runtime* (not by this feature):
                                          #   - values.yaml (mikebom.image)
                                          #   - crds/namespacescan.kusari.dev_v1.yaml (regenerated)
                                          #   - Chart.yaml (version + appVersion)

crates/operator/
├── src/crds/namespace_scan.rs           # UNCHANGED structure. Doc-comment mikebom tag string touched at runtime.
├── src/scan_job/mod.rs                  # UNCHANGED structure. Unit test fixtures touched at runtime.
└── tests/
    └── nightly_version_bump.rs          # NEW: verifies the shell bump script's version arithmetic matches Rust's understanding. Reads Chart.yaml, asserts current version is a well-formed v0.1.0-alpha.N.

docs/
└── release-runbook.md                   # MODIFY: append §5 "Nightly rebuild workflow" — dry-run rehearsal steps, first-run inspection checklist, hold/override procedures, known-bad clear procedure.
```

**Structure Decision**: The feature adds two new workflows and four new
scripts under `.github/`. It does NOT restructure any existing directory.
The existing `release.yml` and `check-versions.sh` are consumed as
external contracts (unchanged). This isolation matches feature 010's
structure and keeps CI/CD infra separable from Rust code.
