# Feature Specification: Nightly Mikebom Rebuild

**Feature Branch**: `011-nightly-mikebom-rebuild`

**Created**: 2026-07-10

**Status**: Draft

**Input**: User description: "let's create a nightly release job that rebuilds based on the latest released alpha of mikebom"

## Clarifications

### Session 2026-07-10

- Q: How should failed nightly runs be reported to maintainers? → A: GitHub issue (with de-dup on failure signature) + workflow annotation on the failing run. FR-012 updated; FR-016 added for de-dup semantics.
- Q: When a mikebom alpha fails operator CI and mikebom hasn't cut anything newer, what should the nightly do on subsequent nights? → A: Skip the alpha until a newer one appears (or maintainer clears the "known-bad" marker). FR-017 added.
- Q: If night 1's bump PR is still open when night 2's cron fires, what should the workflow do? → A: Skip the run; leave the existing PR untouched; record "waiting on PR #X" in the run summary. FR-018 added.

## User Scenarios & Testing

### User Story 1 - Operator tracks mikebom alpha releases without manual bumps (Priority: P1)

The operator's pinned mikebom image tag drifts behind mikebom's release cadence
every time a maintainer forgets to run the manual bump. As the operator's
consumer, I want a scheduled job that notices when mikebom has cut a new alpha,
bumps every live reference in the operator repo, cuts a matching operator
release, and publishes signed artifacts — all without me touching the repo.

**Why this priority**: This is the entire feature. Without P1, the maintainer
is still doing the manual bump-and-release dance we did earlier this session
for `v0.1.0-alpha.57`. The whole point is to remove that toil.

**Independent Test**: Manually trigger the workflow (via `workflow_dispatch`) at
a moment when mikebom has published a newer alpha than the operator's current
pin. Verify that the workflow completes with a new operator tag pushed, a new
signed image on `ghcr.io/kusari-oss/mikebom-operator`, and a new signed chart on
`ghcr.io/kusari-oss/charts/mikebom-operator`, all pinning the newer mikebom
alpha. Trigger it a second time immediately: verify it no-ops.

**Acceptance Scenarios**:

1. **Given** mikebom has published `v0.1.0-alpha.58` and the operator's current
   pin is `v0.1.0-alpha.57`, **When** the nightly job fires, **Then** the
   workflow bumps every live reference from `.57` to `.58`, computes the next
   operator version, runs the operator's test suite, and delivers a new release
   pinning `v0.1.0-alpha.58`.
2. **Given** the current operator pin is `v0.1.0-alpha.58` and mikebom has not
   cut a newer alpha, **When** the nightly job fires, **Then** the workflow
   completes as a no-op with a run summary line stating "no bump needed, still
   pinned to alpha.58" and no new release is cut.
3. **Given** mikebom has cut multiple newer alphas since the last operator pin
   (e.g., current pin `.57`, mikebom published `.58` and `.59`), **When** the
   nightly job fires, **Then** the workflow selects the highest semver-valid
   alpha (`.59`) and skips the intermediate one.

---

### User Story 2 - Maintainer sees actionable failure signals when the nightly breaks (Priority: P2)

A silent nightly failure is worse than no nightly at all — the operator falls
behind mikebom while everyone assumes it's tracking. As the maintainer on-call,
I want to know within one business day whenever the nightly failed, with a
direct link to the failing job and the specific step that broke.

**Why this priority**: P1 delivers value only when observable. Without P2, a
CI regression in the middle of the workflow (test failure, image build failure,
cosign failure, mikebom manifest missing) leaves the operator pinned to a stale
alpha indefinitely with no signal. P2 is the safety net that makes P1
trustworthy.

**Independent Test**: Introduce a synthetic failure (e.g., point the mikebom
tag lookup at a nonexistent repo, or fail one of the operator's unit tests on
a feature branch). Run the workflow. Verify that a GitHub issue is opened (or
the workflow run's summary is annotated) with a link to the failing step and
enough context to triage.

**Acceptance Scenarios**:

1. **Given** the mikebom tag lookup step returns an error (network failure or
   404), **When** the nightly job runs, **Then** a maintainer-visible signal
   fires (GitHub issue, annotation, or equivalent) linking to the failing run
   and no operator release is cut.
2. **Given** the operator's unit tests fail after the mikebom bump, **When**
   the nightly job runs, **Then** the workflow aborts before the release step,
   surfaces the failing test names in the maintainer-visible signal, and does
   NOT tag or push anything.

---

### User Story 3 - Maintainer triggers the same logic on demand (Priority: P3)

Sometimes mikebom cuts an alpha mid-day and the maintainer wants the operator
to catch up before the next nightly window. As a maintainer, I want to
invoke the exact same logic manually, without duplicating it in a separate
one-off workflow.

**Why this priority**: Nice-to-have. The scheduled run handles the 99% case;
this covers the impatient-maintainer edge case. Without it, the maintainer
falls back to the manual bump we just did, which defeats the automation goal
in a mild way but doesn't break anything.

**Independent Test**: Trigger the workflow via GitHub's UI `workflow_dispatch`
button (or `gh workflow run`). Verify it executes the same detection + release
logic as a scheduled run — including no-op behavior when mikebom hasn't moved.

**Acceptance Scenarios**:

1. **Given** a maintainer clicks "Run workflow" from the GitHub UI, **When**
   the workflow runs, **Then** it produces the same behavior as a scheduled
   run for the same repo state.

---

### Edge Cases

- **Concurrent release in flight**: If the existing release pipeline is
  mid-run when the nightly fires, the nightly MUST NOT push a competing tag
  or clobber the in-progress release.
- **Non-alpha mikebom versions**: If mikebom publishes `v0.1.0-beta.1` or
  `v0.1.0`, the nightly ignores it — this feature is scoped to alpha tracking.
  Cutting the operator over to beta or stable is a separate, deliberate
  decision by a maintainer.
- **Missing image manifest**: If the newest mikebom GitHub release is tagged
  but the corresponding `ghcr.io/kusari-oss/mikebom` image manifest isn't yet
  published (race between GitHub release and image push), the nightly MUST
  detect this and defer to the next run rather than release against a
  nonexistent image.
- **Downgrade**: If the newest visible mikebom alpha is *lower* than the
  currently pinned one (e.g., mikebom yanked a release), the nightly MUST NOT
  bump downward. It logs a warning and no-ops.
- **Operator test failure with new mikebom**: If unit tests fail against the
  bumped mikebom, the release step MUST NOT execute. The workflow records the
  failure in a way the maintainer will notice (User Story 2).
- **Operator code changed since last release without a bump**: If someone
  merged operator changes without cutting a release, the nightly does not
  cover that case — it only responds to mikebom drift, not operator drift.
  This is a documented non-goal.
- **Nightly fires with no prior operator tag**: On a fresh repo (no
  `v0.1.0-alpha.*` tag yet), the nightly MUST detect this and no-op with a
  clear message rather than guess at the initial version.

## Requirements

### Functional Requirements

- **FR-001**: The system MUST run on a nightly schedule (default: 03:17 UTC
  daily — off-hour to avoid collision with typical Kusari release windows;
  see plan.md and contracts/nightly-workflow.md for the exact cron) and MUST
  also be invocable on demand.
- **FR-002**: The system MUST read the operator's currently pinned mikebom
  alpha tag from the operator repo's canonical source of truth (the Helm
  chart's default `mikebom.image` value) — not from a duplicated pin.
- **FR-003**: The system MUST query mikebom's public release surface (GitHub
  Releases on `kusari-oss/mikebom` and/or the `ghcr.io/kusari-oss/mikebom`
  container registry) and MUST select the highest semver-valid tag matching
  the `v0.1.0-alpha.N` shape.
- **FR-004**: The system MUST verify that a multi-arch image manifest exists
  on `ghcr.io/kusari-oss/mikebom` for the selected tag before proceeding.
- **FR-005**: When the selected mikebom alpha is strictly greater than the
  currently pinned one, the system MUST update every live reference in the
  operator repo from the old tag to the new tag. "Live references" means
  every file that the manual v0.1.0-alpha.57 bump touched — the Helm chart
  defaults + generated CRD, CRD source doc-comments, unit test fixtures,
  E2E test fixtures, user-facing docs, and top-level examples. Frozen
  historical spec artifacts under `specs/003-*` and `specs/004-*` MUST NOT
  be touched.
- **FR-006**: When the selected mikebom alpha equals or is less than the
  currently pinned one, the system MUST no-op cleanly and record the reason
  in the run summary. No commits, no tags, no releases.
- **FR-007**: When bumping, the system MUST compute the next operator
  version by incrementing the operator repo's own `v0.1.0-alpha.N` counter
  based on existing git tags — independent of mikebom's counter. The operator
  version is not slaved to mikebom's version number.
- **FR-008**: When bumping, the system MUST update the operator's version in
  structural lockstep across `Cargo.toml [workspace.package].version`,
  `charts/mikebom-operator/Chart.yaml` (both `version` and `appVersion`),
  per Constitution VII (Helm Chart Lockstep).
- **FR-009**: The system MUST run the operator's build + unit test suite
  against the bumped state before proceeding to the release step. A failing
  test suite MUST abort the run before any tag is pushed.
- **FR-010**: The system MUST NOT run concurrently with an in-progress
  release pipeline invocation. If a release is in progress, the nightly MUST
  either queue behind it or exit without action, without leaving the repo in
  a partially-bumped state.
- **FR-011**: The system MUST NEVER downgrade the pinned mikebom version.
  If the selected tag is lower than the current pin (e.g., yank), the run
  logs a warning and no-ops.
- **FR-012**: On any failure — build failure, test failure, missing image
  manifest, GitHub API error, tag lookup error — the system MUST emit
  maintainer-visible signals via two channels: (1) a workflow annotation on
  the failing run (in-run visibility when browsing Actions history) AND (2) a
  GitHub issue filed on the operator repo (persistent, searchable audit
  trail). Both channels MUST identify (a) the workflow run URL, (b) the
  specific step that failed, and (c) whether the repo state is clean or
  partially modified.
- **FR-013**: The system MUST support manual triggering with the same code
  path as the scheduled trigger (no duplication of detection or release
  logic).
- **FR-014**: On success, the system MUST leave the repo's default branch in
  a state where a subsequent nightly run (with no mikebom drift) is a
  guaranteed no-op — i.e., the bump commit and release tag MUST land on the
  default branch (or the trunk equivalent) rather than on a dangling branch.
- **FR-018**: Before opening a new bump PR, the system MUST check for
  existing OPEN bump PRs authored by prior nightly runs (identified by a
  well-known label or branch-name prefix). If any exist, the system MUST
  no-op the current run with a summary line "waiting on PR #X" and MUST NOT
  open a new PR, modify the existing one, or push new commits to it. When
  the open PR is merged, closed, or resolved, subsequent nightly runs
  resume normal detection + release behavior.
- **FR-017**: The system MUST maintain a durable in-repo record of mikebom
  alphas that failed operator CI (the "known-bad" set). When a build or test
  step fails against a bumped mikebom alpha, the system MUST add that alpha
  tag to the known-bad set. Before opening a bump PR on a subsequent run,
  the system MUST check whether the highest available mikebom alpha is in
  the known-bad set; if so, the system MUST no-op and record the reason in
  the run summary (e.g., "skipping v0.1.0-alpha.58, marked known-bad on
  2026-07-10"). Entries older than or equal to the currently pinned alpha
  MAY be pruned. Maintainers MUST be able to clear entries from the set
  through normal repo access (e.g., editing the state file via PR).
- **FR-016**: The system MUST de-duplicate failure issues. Before filing a
  new failure issue, the system MUST search for an existing OPEN issue
  matching the same failure signature (defined as the tuple of
  `(failing_step_name, error_class)`). If a match is found, the system MUST
  append a comment on the existing issue linking to the newest failing run
  instead of filing a duplicate. Only when no open matching issue exists
  MUST the system file a new one.
- **FR-015**: The system MUST deliver releases via a hybrid PR-then-tag mode:
  (a) open a pull request against the operator repo containing the bump
  commit; (b) require the existing CI checks and Kusari Inspector to pass
  against the PR; (c) auto-merge the PR only when both are green; (d) on
  merge, follow-up automation pushes the operator release tag, which
  triggers the existing release pipeline (`.github/workflows/release.yml`)
  to build, sign, attest, and publish the artifacts. If CI or Kusari
  Inspector fails on the PR, the PR MUST remain open (not auto-merged), and
  the failure MUST surface as a maintainer-visible signal per FR-012.

## Success Criteria

### Measurable Outcomes

- **SC-001**: When mikebom publishes a new alpha and none of its content is
  incompatible with the operator, the operator's tracked pin catches up within
  24 hours at least 95% of the time.
- **SC-002**: A no-op nightly run (mikebom hasn't moved) completes in under 5
  minutes end-to-end.
- **SC-003**: A full rebuild nightly run (bump → build → test → release)
  completes in under 45 minutes end-to-end, in line with the existing manual
  release pipeline's observed duration.
- **SC-004**: Every failed nightly run produces a maintainer-visible signal
  that the maintainer notices within one business day at 100% rate.
- **SC-005**: The nightly MUST NEVER produce a released operator artifact
  that pins a mikebom alpha whose image manifest is unreachable at the moment
  of release — i.e., zero "released against a nonexistent image" incidents.
- **SC-006**: Over a rolling 30-day window, the operator's pinned mikebom
  alpha MUST NEVER lag behind mikebom's latest alpha by more than 48 hours,
  excluding maintainer-acknowledged holds.

## Assumptions

- The operator remains on `v0.1.0-alpha.*` for the lifetime of this feature.
  A cutover to beta or stable is out of scope and would trigger a spec
  revision.
- Maintainers trust mikebom's alpha releases enough to auto-consume them.
  Manual gating of individual alphas is explicitly out of scope; if a specific
  alpha is known-bad, the maintainer intervenes by yanking or by holding the
  workflow.
- The operator's existing release pipeline (`.github/workflows/release.yml`)
  is the canonical release delivery mechanism. This feature is a driver on
  top of it, not a replacement.
- The `ghcr.io/kusari-oss/mikebom` registry and the `kusari-oss/mikebom`
  GitHub repo are both reachable from GitHub Actions runners during the run
  window.
- The set of "live references" that need bumping is small enough (~12 files)
  and stable enough that a rule-based discovery pass (e.g., grep for the old
  tag, exclude `specs/003-*` and `specs/004-*`) is reliable. If new
  frozen-artifact directories are added later, they are added to the exclude
  list.
- The operator's own `v0.1.0-alpha.N` counter is monotonically incrementing
  from existing git tags. No parallel branches produce colliding alpha
  numbers.

## Resolved Clarifications

### Question 1: Release delivery mode — RESOLVED (Hybrid PR-then-tag)

**Decision**: The nightly opens a PR containing the bump commit, requires CI
+ Kusari Inspector to pass, auto-merges the PR when both are green, and pushes
the release tag via follow-up automation on merge. The existing release
pipeline fires on the tag push. See FR-015 for the normative statement.

**Rationale**: Balances autonomy (no human bottleneck on the happy path) with
auditability (every bump is a PR that CI and Kusari Inspector gate before it
lands). Aligns with the memory guidance to pin third-party deps by default and
let Kusari Inspector enforce security on every PR — the nightly is just
another PR author.
