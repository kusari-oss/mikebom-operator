# Feature Specification: Schedule honoring (cron + interval)

**Feature Branch**: `009-schedule-honoring`

**Created**: 2026-06-29

**Status**: Draft

**Input**: User description: "schedule honoring cron and interval (feature 009)"

## User Scenarios & Testing *(mandatory)*

### User Story 1 — Cron-scheduled re-scan (Priority: P1)

A cluster admin applies a `NamespaceScan` CR with `spec.schedule.cron: "0 */6 * * *"` (every 6 hours). The first reconcile spawns scan Jobs that complete (feature 008 transitions the CR to `Ready=True / reason=ScanCompleted`). Six hours later, the operator re-scans automatically — fresh SBOMs are produced for every in-scope image, and the CR's `status.lastScanCompletedAt` advances to the new completion time. The admin doesn't have to touch the CR between scans.

**Why this priority**: This is the core feature. Up through feature 008, a CR that reached `ScanCompleted` would stay there forever — the SBOM manifest got stale the instant a new image was pulled or a vulnerability was disclosed. P1 because (a) "stale SBOM that says you're safe" is worse than "no SBOM", and (b) the schedule fields have been in the CRD since v0.1 without any honoring behavior, so the user-facing contract has been lying.

**Independent Test**: Apply a CR with `cron: "*/2 * * * *"` (every 2 minutes — tight bound for testing). Wait for the first scan to complete. Within ~2 minutes (+ a small jitter budget), observe that scan Jobs spawn again and the CR transitions back through `Scanning → ScanCompleted`, with `lastScanCompletedAt` advancing.

**Acceptance Scenarios**:

1. **Given** a CR with `cron: "*/5 * * * *"` whose first scan completed at 14:00:30 (`status.lastScanCompletedAt = "2026-06-29T14:00:30Z"`), **When** wall clock reaches 14:05:00 (the next cron tick), **Then** within ~30 seconds the operator re-scans: completed Jobs from the prior scan are removed and `ensure_jobs` (feature 007) creates fresh Jobs for the in-scope image set.
2. **Given** a CR at `ScanCompleted` whose scheduled re-scan window is still in the future, **When** the operator's periodic reconcile fires, **Then** the CR's status MUST remain at `Ready=True / reason=ScanCompleted` and no Jobs are touched.
3. **Given** a CR that has just re-scanned (Jobs respawned, fresh `ScanCompleted` written), **When** the admin reads `status.scannedImages[]`, **Then** each previously-scanned image's entry shows an updated `completedAt` (the merge-by-imageRef semantics from feature 008 § FR-015 produce one entry per image, with the freshest timestamp).

---

### User Story 2 — Interval-scheduled re-scan (Priority: P1)

A cluster admin who doesn't want to think about cron syntax uses `spec.schedule.interval: "6h"` (Go-style duration) instead. The semantics are the same as US1: every 6 hours after the last successful scan, the operator re-scans. The interval is measured from `status.lastScanCompletedAt`, not from CR creation, so a slow first scan doesn't shift the cadence forever.

**Why this priority**: P1 (tied with US1) because the CRD schema has supported `interval` since v0.1 alongside `cron`. Shipping cron without interval would force every admin to translate "6h" into `"0 */6 * * *"`, which is needless friction.

**Independent Test**: Apply a CR with `interval: "2m"`. Wait for the first scan to complete. Within 2 minutes + ~30s of `lastScanCompletedAt`, observe the same re-scan transition as US1.

**Acceptance Scenarios**:

1. **Given** a CR with `interval: "1h"` whose `lastScanCompletedAt` is `2026-06-29T14:00:00Z`, **When** wall clock reaches `15:00:00`, **Then** within ~30 seconds the operator re-scans (completed Jobs removed, fresh Jobs spawned).
2. **Given** a CR with `interval: "10m"` that has not yet completed its first scan, **When** the first scan is still in `Scanning`, **Then** the schedule does NOT fire a second concurrent scan — the operator MUST NOT spawn a new generation while the prior generation has Jobs that are neither succeeded nor finally failed.
3. **Given** a CR whose interval is edited from `1h` to `15m`, **When** the next reconcile fires after the edit, **Then** the operator uses the new interval against the existing `lastScanCompletedAt` (so the next scan fires 15 min after the last completion, not 1 h).

---

### User Story 3 — Operator restart catches up on missed schedules (Priority: P2)

A cluster admin's operator pod crashes overnight (or the chart upgrade restarts it). While the operator was down, multiple scheduled scan windows passed. On recovery, the operator MUST NOT spawn a *separate* scan for each missed window — that would generate dozens of redundant Jobs and exhaust quotas. Instead, it fires **exactly one catch-up scan** per CR whose schedule has elapsed since `lastScanCompletedAt`.

**Why this priority**: P2 because the operator works without it for the steady state — the *next* scheduled window after recovery will fire correctly. The missed-window catch-up only matters for the recovery instant, and most clusters have minute-to-minute schedules, not high-frequency ones. But shipping without it means an operator restart "skips" a scan, which surprises admins who expect the schedule to fire every interval.

**Independent Test**: Apply a CR with `interval: "1m"`. Wait for it to reach `ScanCompleted`. Stop the operator (`kubectl scale deployment mikebom-operator --replicas=0`). Wait 5 minutes (= 5 missed schedule windows). Restart (`kubectl scale ... --replicas=1`). Within 30 seconds of the operator becoming Ready, observe that exactly **one** re-scan fires — not five.

**Acceptance Scenarios**:

1. **Given** an operator that was down for 3 missed schedule windows on a CR, **When** the operator restarts, **Then** the operator MUST fire exactly one catch-up scan on the first reconcile after recovery and resume normal cadence afterward.
2. **Given** a CR whose previous scan failed (`reason=ScanFailed`) and whose schedule says the next scan is due, **When** the operator reconciles, **Then** the failed-Job state from the previous scan is NOT lost: the operator re-spawns scan Jobs (feature 007 idempotent path — same Job names overwrite via the 409 path), and the CR transitions to `Scanning` again. The admin can see both the old `ScanFailed` lastTransitionTime in their monitoring and the new `Scanning` state.

---

### Edge Cases

- **Both `cron` and `interval` set on the same CR**: the operator MUST treat this as invalid. The condition transitions to `Ready=False, reason=InvalidSpec` with a message naming the conflict. (Feature 002's `InvalidSpec` reason is extended to cover schedule validity.)
- **Neither `cron` nor `interval` set**: the operator MUST treat this as `InvalidSpec` (the CRD schema allows both fields to be unset, but the spec requires exactly one). Same message format.
- **`cron` field contains an unparseable expression** (e.g., `"every 6 hours please"`): `InvalidSpec` with a message naming the parse error.
- **`interval` field contains a non-duration string** (e.g., `"6 hours"` instead of `"6h"`): `InvalidSpec`.
- **`interval` is zero or negative** (e.g., `"0s"`, `"-1h"`): `InvalidSpec`. Zero would cause infinite-loop re-scanning.
- **`interval` is shorter than the minimum sensible budget** (e.g., `"500ms"`): `InvalidSpec`. The operator MUST enforce a minimum of `1m` (60 seconds) to prevent pathological scheduling that hammers the cluster.
- **Re-scan fires while previous scan is still `Scanning`** (long-running mikebom-scan, or backed-up Jobs): the operator MUST NOT spawn a second concurrent generation. The schedule check is gated on `status.conditions[Ready].reason ∈ {ScanCompleted, ScanFailed}` — only terminal scan states permit re-scan.
- **CR has `scannedImages[]` populated but `lastScanCompletedAt` is unset** (data corruption or migration edge case): the operator MUST treat `lastScanCompletedAt` as `cr.metadata.creationTimestamp` for schedule arithmetic — never re-scan from the Unix epoch, which would fire immediately and forever after.
- **Cron schedule next-fire-time is in the past** (e.g., admin edits `cron` after a long downtime): the operator MUST fire exactly one catch-up scan, then resume normal cadence from `now`, not from the missed window.
- **Operator's wall clock drifts** (NTP misconfig): re-scan timing follows the local wall clock. The operator does not attempt to validate against an external time source. Admins responsible for cluster NTP hygiene.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: For every `NamespaceScan` CR with a valid spec and `status.conditions[Ready].reason ∈ {ScanCompleted, ScanFailed}` (terminal scan states from feature 008), the operator MUST compute a "next scheduled scan time" from `spec.schedule` + `status.lastScanCompletedAt`. When wall-clock time has reached or passed the next scheduled time, the operator MUST initiate a re-scan.
- **FR-002**: A re-scan MUST consist of (a) deleting Jobs owned by the CR that have `.status.succeeded >= 1` OR are finally failed, then (b) letting feature 007's `ensure_jobs` create fresh Jobs idempotently for the current in-scope image set. The operator MUST NOT spawn duplicate Jobs (feature 007 invariant); the deletion step ensures the new generation has distinct names from the prior generation's terminal Jobs (which are removed before `ensure_jobs` runs).
- **FR-003**: The operator MUST honor `spec.schedule.cron` as a standard 5-field cron expression (minute hour day-of-month month day-of-week). Invalid expressions surface as `InvalidSpec`.
- **FR-004**: The operator MUST honor `spec.schedule.interval` as a Go-style duration string (e.g., `"6h"`, `"30m"`, `"24h"`). Invalid or zero/negative durations surface as `InvalidSpec`. Intervals shorter than 1 minute surface as `InvalidSpec`.
- **FR-005**: A CR with both `cron` AND `interval` set, OR neither set, MUST surface `Ready=False / reason=InvalidSpec`. Feature 002's `InvalidSpec` reason is extended to cover schedule validity; the message MUST name which combination of fields is offending.
- **FR-006**: A re-scan MUST NOT fire while the previous scan is still in progress. The schedule check MUST be gated on `status.conditions[Ready].reason ∈ {ScanCompleted, ScanFailed}` — only terminal states unlock the schedule.
- **FR-007**: For a CR whose `lastScanCompletedAt` is set, the next scheduled time MUST be computed relative to `lastScanCompletedAt`: for `interval`, it's `lastScanCompletedAt + interval`; for `cron`, it's the next cron tick after `lastScanCompletedAt`. For a CR whose `lastScanCompletedAt` is unset (e.g., on a CR that has never completed a scan), the operator MUST treat `lastScanCompletedAt` as the CR's `metadata.creationTimestamp` for the calculation.
- **FR-008**: After an operator restart with one or more missed scheduled windows for a CR, the operator MUST fire exactly **one** catch-up scan on the first reconcile after recovery. The operator MUST NOT iterate through missed windows.
- **FR-009**: When a re-scan completes, `status.scannedImages[]` MUST contain one entry per scanned image with the most recent `completedAt`. Feature 008's `merge_scanned_images_append_only` semantics apply: same `image_ref` → newest-wins, so a re-scan updates timestamps but never duplicates entries.
- **FR-010**: The operator SHOULD expose the next scheduled re-scan time on the CR's status (additive field `status.nextScheduledScanAt: Option<String>` in RFC 3339). This is admin-visible signal ("when's my next scan?") and MUST be updated on every reconcile that produces a stable terminal scan state. (Constitution IV: additive to v1alpha1.)
- **FR-011**: The operator MUST NOT actively delete Jobs that are still in progress (`status.succeeded < 1` AND not finally failed). Deletion at the schedule boundary only targets terminal Jobs.
- **FR-012**: Feature 007 and feature 008 tests MUST continue to pass unchanged. The re-scan path is additive: it deletes terminal Jobs and re-enters feature 007's `ensure_jobs` path.
- **FR-013**: Constitution IV (CRD backward compat): adding `status.nextScheduledScanAt` is the only schema addition in this feature. No other fields change shape. Feature 001's drift check verifies.
- **FR-014**: The operator MUST log every schedule trigger (CR name, last-completion timestamp, next-scheduled timestamp, decision: fire/defer) at `info` level. The logs are the primary debugging signal when a re-scan doesn't fire when an admin expected it to.
- **FR-015**: An admin editing `spec.schedule` (e.g., changing `interval` from `"1h"` to `"15m"`) MUST see the new schedule honored on the next reconcile after the edit, without restarting the operator.

### Key Entities

- **Schedule expression (CR field)**: existing `spec.schedule.{cron, interval}` from feature 001's CRD. Feature 009 makes these fields *consequential* for the first time.
- **Next scheduled time (computed)**: a function of `(spec.schedule, lastScanCompletedAt)`. Computed every reconcile; surfaced in `status.nextScheduledScanAt` for admin visibility.
- **Terminal Job set (existing, now deletion target)**: Jobs owned by the CR whose `.status.succeeded >= 1` or whose `.status.failed > backoffLimit`. Feature 008 reads these for aggregation; feature 009 deletes them at re-scan boundaries.
- **`status.nextScheduledScanAt` (new CRD field)**: optional RFC 3339 string. Additive to v1alpha1.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A CR with `cron: "*/2 * * * *"` re-scans within 30 seconds of every 2-minute cron tick once it has reached `ScanCompleted`. (Verifies FR-001, FR-002, FR-003.)
- **SC-002**: A CR with `interval: "2m"` re-scans within 30 seconds of `lastScanCompletedAt + 2m`. (Verifies FR-001, FR-004, FR-007.)
- **SC-003**: A CR with `cron: "every 6 hours"` (invalid expression) reaches `Ready=False / reason=InvalidSpec` within 10 seconds of being applied, with a message naming the parse error. (Verifies FR-003, FR-005.)
- **SC-004**: A CR with BOTH `cron: "0 * * * *"` AND `interval: "1h"` reaches `Ready=False / reason=InvalidSpec` within 10 seconds. The message names the conflict. (Verifies FR-005.)
- **SC-005**: After an operator pod restart that spans 3 missed schedule windows for a CR with `interval: "1m"`, exactly **1** catch-up scan fires within 30 seconds of operator recovery (verified by counting Job-creation events in operator logs or live Job timestamps). (Verifies FR-008.)
- **SC-006**: An admin editing `spec.schedule.interval` from `"1h"` to `"15m"` on a CR at `ScanCompleted` triggers the next re-scan within 15 minutes (+ 30s budget) of the edit time, not 1 hour. (Verifies FR-015.)
- **SC-007**: A CR's `status.nextScheduledScanAt` field is populated within 5 seconds of every reconcile that produces a `ScanCompleted` or `ScanFailed` state, and the value is in the future relative to `lastScanCompletedAt`. (Verifies FR-010.)
- **SC-008**: A re-scan against a CR with 3 scanned images updates each entry's `completedAt` in `status.scannedImages[]` without growing the array. (Verifies FR-009.)

## Assumptions

- **Cron timezone is UTC**: the operator interprets cron expressions in UTC, not in the cluster's local timezone. Admins crossing timezones can compute UTC equivalents at apply time. The CRD does not currently have a timezone field; adding one is a future feature.
- **Minimum interval is 1 minute**: shorter intervals are rejected as `InvalidSpec`. This is a soft guardrail against pathological scheduling that hammers the cluster. Industry practice (e.g., Kubernetes CronJob) caps at "no more often than per minute"; we adopt the same.
- **Schedule arithmetic precision is ±30 seconds**: SC-001/SC-002 budget the test bound at 30s above the scheduled time. This accommodates the existing 5-minute requeue cadence (which we'll tighten for CRs that have an imminent schedule trigger) plus reconcile-loop latency. Exact-second precision would require a separate scheduling loop, which is out of scope.
- **`lastScanCompletedAt` is the schedule anchor**, not `metadata.creationTimestamp`. This means a CR that takes a long time to complete its first scan doesn't shift the cadence to the *creation* time minus that delay. Once `lastScanCompletedAt` is populated, it's the truth. For a CR with no `lastScanCompletedAt` (never-yet-completed first scan), the anchor falls back to `metadata.creationTimestamp` per FR-007.
- **Re-scan delete-then-spawn is two reconcile cycles**: the first reconcile deletes terminal Jobs (the CR's status reflects this briefly — possibly returning to `Scanning` with 0 owned Jobs in the empty-list row of feature 008's aggregation table); the second reconcile (immediately re-enqueued by the Job-watch event for the deletions) calls `ensure_jobs` which spawns fresh Jobs. The admin sees the transition `ScanCompleted → Scanning → ScanCompleted` cleanly.
- **No concurrent-scan prevention beyond gate**: FR-006 prevents re-scan while Jobs are still progressing. The operator does not queue future scans — if a scan takes longer than the schedule interval (e.g., 10-minute scan on a 5-minute interval), the cadence becomes "as fast as the scan finishes" and the admin's effective schedule is "back-to-back scans." This is a real production concern but a rare misconfiguration; surfacing a warning condition is out of scope for v0.9.
- **Constitution VI E2E**: a new gated kind E2E exercises the cron + interval paths with tight schedules (e.g., 2-minute) and asserts the re-scan transition. Reuses feature 008's `e2e/tests/common/mod.rs` chart-install scaffolding.
- **Cron parsing library**: planning phase will pick a Rust cron library (e.g., `cron` 0.12+ or `croner`). Stable, widely-used libraries exist; this is a planning choice.
- **Helm chart**: no changes. RBAC already grants `batch/v1.jobs:get,list,watch,create,delete` — the existing `delete` verb covers feature 009's terminal-Job cleanup. No new CRD fields require schema updates beyond `status.nextScheduledScanAt` (which is generated from the Rust struct via feature 001's drift check).
- **No manual-trigger API in v0.9**: admins can force a re-scan by editing the CR (any benign change triggers a reconcile, and the next reconcile sees the schedule has elapsed). A dedicated "scan now" action (e.g., an annotation like `kusari.dev/trigger-scan-at: <timestamp>`) is a future feature.
