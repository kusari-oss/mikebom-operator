---

description: "Task list for feature 011: nightly mikebom rebuild"
---

# Tasks: Nightly mikebom rebuild

**Input**: Design documents from `/specs/011-nightly-mikebom-rebuild/`

**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/{nightly-workflow,tag-workflow}.md, quickstart.md

**Tests**: Included. One Rust integration test (regression guard on version arithmetic), plus grep/shellcheck-style polish checks. E2E validation is delegated to `ci.yml`, which fires on the auto-opened bump PR — same discipline as features 001–010 (Constitution VI).

**Organization**: Tasks grouped by user story. US1 is the MVP — detection + bump + PR + auto-merge + tag push, end-to-end. US2 layers on issue-based failure signals with de-dup. US3 completes the workflow_dispatch surface. Polish captures the Rust regression test, shellcheck integration, runbook, SHA-pin verification, and dry-run rehearsal.

## Format: `[ID] [P?] [Story?] Description with file path`

- **[P]**: Can run in parallel (different files, no in-flight dependency).
- **[Story]**: Required on Phase 3+ tasks (US1 / US2 / US3).

## Path Conventions

Two new workflows at `.github/workflows/{nightly-mikebom-bump,tag-on-nightly-merge}.yml`. Four new shell scripts at `.github/scripts/nightly-{detect,bump,open-pr,tag}.sh`. One new Rust integration test at `crates/operator/tests/nightly_version_bump.rs`. Runbook update at `docs/release-runbook.md`. Repo-level configuration (labels) documented in-runbook. All paths repo-relative.

---

## Phase 1: Setup (Repo-level prerequisites)

**Purpose**: One-time repo configuration so the workflows have the labels + branch-protection they depend on.

- [X] T001 Create three GitHub labels on `kusari-oss/mikebom-operator` via `gh label create` (one-time maintainer step, documented in T012's runbook update): `nightly-mikebom-bump` (color `#0e8a16`, description "Auto-opened by nightly-mikebom-bump.yml"), `nightly-mikebom-bump/cleared` (color `#ffd700`, description "Maintainer override — excludes closed PR from known-bad set"), `nightly-mikebom-bump/failure` (color `#d93f0b`, description "Nightly workflow failure — auto-filed with de-dup"). All three are queried by name from `nightly-detect.sh` and `signal_failure` step, so the exact strings matter.

---

## Phase 2: Foundational (Rust regression scaffold)

**Purpose**: Introduce the Rust integration test file BEFORE user-story implementation so any subsequent version-arithmetic bug in `nightly-bump.sh` fails a normal `cargo test --workspace` before it can land in a PR.

- [X] T002 Create `crates/operator/tests/nightly_version_bump.rs` as a scaffold that reads `charts/mikebom-operator/Chart.yaml` (via existing workspace `serde_yaml` dep), asserts `.version` matches regex `^0\.1\.0-alpha\.[0-9]+$`, and asserts `.appVersion == .version`. This scaffold catches Chart.yaml drift on any PR (nightly-opened or otherwise). T010 in Polish adds the real "next-alpha computation" assertion once the shell script's arithmetic is finalized.

**Checkpoint**: After Phase 2, `cargo test --workspace --lib --tests` still passes (Chart.yaml is currently `0.1.0-alpha.1`, matches regex). User-story work can begin.

---

## Phase 3: User Story 1 — Nightly bumps mikebom without maintainer touch (Priority: P1) 🎯 MVP

**Story goal**: Full end-to-end nightly loop — detect the highest mikebom alpha, verify multi-arch manifest, check for stale PRs and known-bad markers, bump all live references, regenerate the CRD, bump operator version in structural lockstep, open a labeled PR, enable auto-merge, and let the tag workflow push the release tag on merge.

**Independent Test** (per quickstart.md §3): manually dispatch `nightly-mikebom-bump.yml` with `dry_run=false` at a moment when mikebom is at a newer alpha than the operator's pin. Within 3 minutes, an auto-opened PR appears matching the schema in `contracts/nightly-workflow.md` "PR body schema". CI runs on the PR (unchanged `ci.yml`); on green + Kusari Inspector green, auto-merge fires. Within 60 seconds of merge, `tag-on-nightly-merge.yml` pushes the `v0.1.0-alpha.<N+1>` tag. `release.yml` fires and produces signed artifacts identical to a manual release.

**Implementation:**

- [X] T003 [P] [US1] Create `.github/scripts/nightly-detect.sh` (POSIX shell, ~120 lines). Reads `charts/mikebom-operator/values.yaml` `.mikebom.image` via `yq`. Queries `gh api repos/kusari-oss/mikebom/releases --paginate --jq` filtered to regex `^v0\.1\.0-alpha\.[0-9]+$`, semver-sorts, picks highest. Runs `docker manifest inspect ghcr.io/kusari-oss/mikebom:<selected>` and pipes to `jq` asserting `mediaType == "application/vnd.oci.image.index.v1+json"` AND both amd64 + arm64 platforms present. Computes known-bad set via the exact `gh pr list` query from research.md §R3. Checks for open bump PRs via `gh pr list --state open --label nightly-mikebom-bump`. Reads existing operator tags via `git tag -l 'v0.1.0-alpha.*' | sort -V | tail -1`. Writes exactly the outputs from `contracts/nightly-workflow.md` step 3 into `$GITHUB_OUTPUT` (`decision`, `current_pin`, `latest_mikebom`, `next_operator_tag`, `open_pr_number`). Writes the decision table into `$GITHUB_STEP_SUMMARY`. Exit 0 for every noop or bump decision; nonzero only on hard failure. **FR-011 downgrade guard**: within the `noop_up_to_date` branch, if the selected mikebom tag's alpha_num is strictly LESS THAN the current pin's alpha_num (yank scenario), emit `echo "::warning::mikebom highest available (${selected}) is lower than current pin (${current_pin}); this looks like a mikebom yank — refusing to downgrade"` before returning the decision. The decision remains `noop_up_to_date` (no separate enum variant needed) but the warning surfaces the anomaly.
- [X] T004 [P] [US1] Create `.github/scripts/nightly-bump.sh` (POSIX shell, ~100 lines). Takes env `MIKEBOM_OLD`, `MIKEBOM_NEW`, `OPERATOR_NEW`. Runs `sed -i` (or `perl -pi -e` for portability) against the exact 12 files listed in `contracts/nightly-workflow.md` step 4 for mikebom-tag replacement (values.yaml + crds/namespacescan.kusari.dev_v1.yaml + crates/operator/src/crds/namespace_scan.rs + crates/operator/src/scan_job/mod.rs + docs/crd-reference.md + examples/namespacescan.yaml + 6 e2e/tests/*.rs files — the same 12 the manual v0.1.0-alpha.57 bump touched), replacing `$MIKEBOM_OLD` → `$MIKEBOM_NEW`. Note: the CRD YAML's sed edit is transient — the next step (regenerate) overwrites it — but keeping it in the sed pass guarantees consistency if regeneration is temporarily broken. Regenerates the CRD via `cargo run --release --bin mikebom-operator-ctl -- crd > charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml`. Bumps `Cargo.toml` `[workspace.package].version` via `sed` (NOT via mikebom-tag replacement — this is a separate version bump). Bumps `charts/mikebom-operator/Chart.yaml` `version` AND `appVersion` via `yq -i`. Runs `cargo fmt -- --check` + `cargo build --workspace` + `cargo test --workspace --lib`. Exits nonzero on any failure — the `nightly-mikebom-bump.yml` workflow's `on failure` handler then routes to `signal_failure`.
- [X] T005 [P] [US1] Create `.github/scripts/nightly-open-pr.sh` (POSIX shell, ~80 lines). Takes env `MIKEBOM_NEW`, `OPERATOR_NEW`, `GH_TOKEN`, `PR_LABEL=nightly-mikebom-bump`, `RUN_URL`. Runs `git config user.name/email` for `github-actions[bot]`. Creates branch `automation/nightly-bump/${MIKEBOM_NEW}`. Stages exactly the paths from T004's file list plus Cargo.toml + Chart.yaml + the regenerated CRD. Commits with the exact template from `contracts/nightly-workflow.md` step 5 (the `Nightly-Bump-Target: <MIKEBOM_NEW>` trailer MUST appear on its own line at the end of the message — `tag-on-nightly-merge.yml`'s detection depends on this). Pushes. Writes the PR body to `/tmp/pr-body.md` using the schema from `contracts/nightly-workflow.md`. Calls `gh pr create` with `--title`, `--body-file /tmp/pr-body.md`, `--label ${PR_LABEL}`, `--base main`. Captures the PR number from the return; calls `gh pr merge <number> --auto --squash --delete-branch`. Exits nonzero if any step fails.
- [X] T006 [P] [US1] Create `.github/scripts/nightly-tag.sh` (POSIX shell, ~40 lines). Reads `charts/mikebom-operator/Chart.yaml` `.appVersion` via `yq`. Constructs `operator_tag = "v${app_version}"`. Runs `git rev-parse "refs/tags/${operator_tag}"` — if the tag already exists, exits 0 with a `::warning::` annotation (idempotency per `contracts/tag-workflow.md` step 4). Otherwise configures git identity for `github-actions[bot]`, runs `git tag -a "${operator_tag}" -m "Nightly release ${operator_tag}"`, pushes to origin. Exits 0 on success, nonzero on `git push` failure. Used by `tag-on-nightly-merge.yml` in T008 — NOT by the nightly workflow.
- [X] T007 [US1] Create `.github/workflows/nightly-mikebom-bump.yml` matching every clause of `contracts/nightly-workflow.md` (triggers block including `schedule: - cron: '17 3 * * *'` and `workflow_dispatch` with `dry_run: bool default: false`; workflow-level `permissions: contents: write, pull-requests: write, issues: write` — NO `id-token`; `concurrency: group: nightly-mikebom-bump, cancel-in-progress: false`). Single job with steps 1–5 from the contract: SHA-pinned `actions/checkout` (`fetch-depth: 0`), SHA-pinned `mikefarah/yq` install, `run: bash .github/scripts/nightly-detect.sh`, conditional `run: bash .github/scripts/nightly-bump.sh` (guarded on `steps.detect.outputs.decision == 'should_bump'`), conditional `run: bash .github/scripts/nightly-open-pr.sh` (guarded on `should_bump` AND `!inputs.dry_run`). SHA pins reuse the SHAs already in `release.yml` — do NOT introduce new SHAs. Step 6 (`signal_failure`) is out-of-scope for US1 and lands in US2's T009. Dry-run branch (skip push, PR create, auto-merge, print `::notice::` describing what would have happened): lands here and covers US3's T010 by construction.
- [X] T008 [US1] Create `.github/workflows/tag-on-nightly-merge.yml` matching every clause of `contracts/tag-workflow.md` (trigger `on: push: branches: [main]`; workflow-level `permissions: contents: write`; `concurrency: group: tag-on-nightly-merge, cancel-in-progress: false`). Single job with steps 1–5 from the contract: SHA-pinned `actions/checkout` (`fetch-depth: 0`, `fetch-tags: true`), inline `detect_nightly_bump` step reading `git log -1 --format=%B | grep '^Nightly-Bump-Target:'`, SHA-pinned `mikefarah/yq` install, inline `resolve_operator_tag` + `check_tag_idempotency` + `run: bash .github/scripts/nightly-tag.sh`. Non-nightly commits short-circuit at step 2 with a `::notice::`, workflow succeeds no-op. All output-passing between steps uses `$GITHUB_OUTPUT` per the contract's exact `echo "key=val" >> "$GITHUB_OUTPUT"` idiom (no deprecated `set-output` syntax).

**Checkpoint**: After Phase 3, `bash .github/scripts/nightly-detect.sh` runs cleanly locally (against production data) and the workflow file lints via `actionlint` if installed. A `workflow_dispatch` run with `dry_run=true` executes detection + bump-in-scratch + prints the "would have done X" notices without opening a PR or leaving remote state. This is the US1 MVP — everything downstream (auto-merge, tag push, release) is triggered by external systems (GitHub, `ci.yml`, `release.yml`) that already exist.

---

## Phase 4: User Story 2 — Failure signals (Priority: P2)

**Story goal**: Every failed nightly run surfaces via BOTH a workflow annotation AND a GitHub issue, with de-dup on `(failing_step, error_class)` so persistent failures don't spam a fresh issue every night.

**Independent Test** (per spec.md US2 acceptance scenario 1): dispatch the workflow with a synthetic failure injected (e.g., temporarily `chmod -x` one of the scripts before dispatch, or set `MIKEBOM_REPO=kusari-oss/mikebom-does-not-exist`). Verify: (a) the workflow run shows a red `::error::` annotation in the summary identifying the failing step; (b) a new GitHub issue is filed with label `nightly-mikebom-bump/failure`, title matching the schema `[nightly] failed at step <name>`, and body containing the run URL + step name + repo-state statement; (c) re-run the same failure — no second issue is filed; instead, a comment is appended to the existing issue.

**Implementation:**

- [X] T009 [US2] Extend `.github/workflows/nightly-mikebom-bump.yml` with a composite `signal_failure` step at the end of the job, guarded by `if: failure()`. Implement inline per `contracts/nightly-workflow.md` step 6: build `failure_signature = "${GITHUB_JOB}_${step_name}_${exit_code_class}"` (where `error_class` is a coarse bucket: `network` for non-zero curl/gh api exit, `test` for cargo test failures, `manifest` for docker manifest inspect failures, `push` for git push failures, `unknown` for anything else). Compute repo-state via `if test -z "$(git status --porcelain)"; then repo_state=clean; else repo_state=partially-modified; fi` — the issue body says which. Run `gh issue list --state open --label nightly-mikebom-bump/failure --search "in:title <step_name>" --json number,title,body`; if a match is returned, `gh issue comment <n> --body "<annotation with new run URL + repo-state>"`; else `gh issue create --title "[nightly] failed at ${step_name}" --body-file /tmp/issue-body.md --label nightly-mikebom-bump/failure`. In parallel, always `echo "::error file=.github/workflows/nightly-mikebom-bump.yml::Nightly failed at ${step_name}. Issue: ${issue_url}"` for in-run annotation. This step MUST NOT itself fail the workflow if `gh issue create` errors (e.g., permissions) — swallow the error and fall back to annotation-only, per the contract's "signal_failure step itself fails" row.

---

## Phase 5: User Story 3 — On-demand manual trigger (Priority: P3)

**Story goal**: A maintainer can invoke the exact same detection + bump logic manually via `gh workflow run` or the GitHub UI. No duplicated code path.

**Independent Test** (per spec.md US3 acceptance scenario): after T007 lands, run `gh workflow run nightly-mikebom-bump.yml` (no `-f dry_run=...`, so it uses the workflow_dispatch default of `false` per the contract) and `gh workflow run nightly-mikebom-bump.yml -f dry_run=true`. Verify both invocations produce the same output structure as a scheduled run (same summary table, same decision), differing only in whether push/PR side effects happen.

**Implementation:**

- [X] T010 [US3] Verify `.github/workflows/nightly-mikebom-bump.yml` (from T007) already exposes the `workflow_dispatch` trigger with the exact input schema from `contracts/nightly-workflow.md` triggers block, including `dry_run: type: boolean default: false`. Verify the dry-run branch inside the workflow uses the same `nightly-detect.sh` invocation and the same output-summary format as the scheduled path (only the `open_pr` step is guarded off). If T007's implementation deviates, fix it here — this task is a validation-and-align, not a fresh add. No new files.

---

## Phase 6: Polish & Cross-Cutting Concerns

- [X] T011 Fill in `crates/operator/tests/nightly_version_bump.rs` (scaffolded in T002) with the real "next-alpha computation matches shell script" assertion. Read `charts/mikebom-operator/Chart.yaml` `.version`, parse the trailing `.<N>` digits, assert `N >= 1`, and construct the string `next = format!("v0.1.0-alpha.{}", N + 1)`. Assert that this next string is well-formed (matches the same regex the shell script uses). This closes the loop: if a future edit to `nightly-bump.sh`'s version arithmetic diverges from the Rust regex, `cargo test --workspace` catches it on any PR.
- [X] T012 Extend `docs/release-runbook.md` with a new "§5 Nightly rebuild workflow" section derived from `specs/011-nightly-mikebom-rebuild/quickstart.md`. Copy §§1–6 of quickstart verbatim (dry-run rehearsal, first-run checklist, common operational actions, trust boundaries, troubleshooting). Cross-link to `release.yml` §4 (existing verification runbook) so the maintainer can follow the same cosign verification steps against nightly-published tags. Include the one-time `gh label create` commands from T001 as a "first-time setup" callout at the top of §5.
- [X] T013 SHA-pin verification. Add a step to `.github/workflows/ci.yml` (or a new `.github/scripts/check-pinned-actions.sh` invoked by the existing CI) that runs `grep -rn "uses: .*@" .github/workflows/nightly-mikebom-bump.yml .github/workflows/tag-on-nightly-merge.yml | grep -vE "@[0-9a-f]{40}"` and fails if any match is found. This enforces the "zero new SHAs" invariant from plan.md — every `uses:` in the new workflows MUST reuse a SHA already pinned elsewhere in the repo.
- [X] T014 Shellcheck integration. Add a step to `.github/workflows/ci.yml` (or an existing lint job) that runs `shellcheck .github/scripts/nightly-*.sh` (POSIX mode, `-s sh`). Fails the CI job on any warning. Covers T003–T006 script quality on every PR going forward.
- [ ] T015 First-run dry-rehearsal. After all above tasks land on a feature branch, run `gh workflow run nightly-mikebom-bump.yml -f dry_run=true --ref 011-nightly-mikebom-rebuild` and record in the PR description: (a) the run URL, (b) the decision + reason, (c) confirmation that no orphan branch appears on `origin` (`git ls-remote --heads origin | grep automation/nightly-bump/` returns nothing), (d) confirmation that no PR was opened (`gh pr list --label nightly-mikebom-bump --state open` empty). This is the human sign-off before the schedule is enabled — same discipline as feature 010's dry-run rehearsal before the first tag push.

---

## Dependencies

**Story completion order** (respecting the P1/P2/P3 priority but noting one implementation-order tweak):

1. Phase 1 (T001) — repo config; can happen before anything else, mostly independent
2. Phase 2 (T002) — Rust scaffold
3. Phase 3 (T003–T008) — US1 MVP
4. Phase 4 (T009) — US2 layers onto T007's workflow
5. Phase 5 (T010) — US3 validates/aligns T007
6. Phase 6 (T011–T015) — polish

**Within Phase 3** (US1):
- T003, T004, T005, T006 are all [P] (four distinct script files, no cross-dependencies)
- T007 needs T003, T004, T005 to exist (references their paths in `run:` steps). Depends on T007 completing before T009 and T010 in Phase 4/5.
- T008 needs T006 to exist. Independent of T007.

**Within Phase 4** (US2) and **Phase 5** (US3):
- Both modify `.github/workflows/nightly-mikebom-bump.yml` — same file. T009 and T010 are sequential (T009 then T010), not [P]. If they run in the same edit session, order doesn't matter; if separate PRs, land T009 first for merge-conflict safety.

**Within Phase 6**:
- T011 depends on T002 (scaffold exists).
- T012 depends on quickstart.md (already exists) and T001 (label commands to copy).
- T013 depends on T007, T008 existing (grep target).
- T014 depends on T003–T006 existing (shellcheck target).
- T015 depends on T007 existing on the branch AND T001 completed (labels must exist for the PR-list query to work).

## Parallel Execution Opportunities

Within US1's Phase 3:

```text
T003 (nightly-detect.sh)   ┐
T004 (nightly-bump.sh)     ├─ Four parallel [P] script authorings.
T005 (nightly-open-pr.sh)  │  Each script is self-contained. Zero shared code.
T006 (nightly-tag.sh)      ┘
                           │
                           ▼
T007 (nightly-mikebom-bump.yml)  ─ needs T003, T004, T005 by path reference
T008 (tag-on-nightly-merge.yml)  ─ needs T006 by path reference
                           │
                           ▼
       [Phase 3 MVP complete — dry-run rehearsable]
```

Within Phase 6, T011, T012, T013, T014 are all touching distinct files and can be [P] once their upstream deps land. T015 is inherently last (it's the end-to-end rehearsal).

## Implementation Strategy: MVP first

**Minimum viable delivery** (US1 + Phase 6 gate T015): the maintainer can dispatch the nightly manually against production repo state, watch the dry-run succeed, then enable the cron. Failure signals in that first cut fall back to GitHub Actions' built-in workflow-failure notification (the maintainer's watching the repo). This is enough for a real first-run smoke test.

**Iteration 2** (US2 + T014 shellcheck): adds durable audit trail via failure issues + shellcheck-gated script quality. Landed as the follow-up PR before enabling the cron schedule.

**Iteration 3** (T011 + T013): closes the last regression gates (Rust test + SHA-pin verification). Trivially small — can land alongside iteration 2 if T007 is stable.

Format validation: every task above starts with `- [ ]`, has a T### ID in `T###` format, carries a `[Story]` label where required (US1/US2/US3 in Phase 3–5, no label in Phase 1/2/6), lists exact repo-relative file paths, and describes a concrete deliverable an implementing agent can complete without further clarification.
