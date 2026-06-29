# Quickstart: Schedule honoring (cron + interval)

Two perspectives: cluster admin (using v0.9) and contributor (extending the
scheduler).

## Cluster admin: upgrading from v0.8 to v0.9

**Chart-side**: no template changes. The CRD's `status` schema gains one new
optional field (`status.nextScheduledScanAt`), which Helm picks up on upgrade
via the regenerated chart YAML:

```sh
helm upgrade mikebom-operator charts/mikebom-operator \
  -n kusari-operator --wait --timeout 60s
```

### What changes for the user

**Before v0.9**: `spec.schedule.cron` and `spec.schedule.interval` were
*documented* fields but had no effect — the operator scanned once and stayed
at `ScanCompleted` forever. To re-scan, an admin had to manually delete the
CR and re-apply, or edit the CR to force a reconcile (which still wouldn't
re-scan — feature 007's idempotency saw the existing Jobs).

**After v0.9**: schedules are honored. A CR with `cron: "0 */6 * * *"` produces
fresh SBOMs every 6 hours, automatically. The CR's status surfaces both the
last completion time and the next scheduled time:

```yaml
status:
  conditions:
    - type: Ready
      status: "True"
      reason: ScanCompleted
      message: "scanned 5 distinct images successfully"
      lastTransitionTime: "2026-06-29T14:00:30Z"
  lastReconciledAt: "2026-06-29T14:30:00Z"
  lastScanCompletedAt: "2026-06-29T14:00:30Z"
  nextScheduledScanAt: "2026-06-29T20:00:30Z"   # NEW: next scan is 6h from last
  scannedImages:
    - imageRef: "nginx:1.27.0"
      sbomLocation: "s3://sboms-prod/team-a/a1b2c3d.json"
      completedAt: "2026-06-29T14:00:25Z"
    # ... more
```

Inspect schedule status via:

```sh
kubectl get namespacescan scan-prod -n kusari-operator \
  -o jsonpath='{.status.nextScheduledScanAt}'
# Prints: 2026-06-29T20:00:30Z
```

### Recipe book

#### Run every 6 hours (cron)

```yaml
spec:
  schedule:
    cron: "0 */6 * * *"
```

Standard 5-field cron: `minute hour day-of-month month day-of-week`.
Interpreted in UTC.

#### Run every 30 minutes (interval)

```yaml
spec:
  schedule:
    interval: "30m"
```

Go-style duration. Measured from `lastScanCompletedAt`, so the cadence is
"30m after the last successful scan."

#### Run nightly at 2 AM UTC

```yaml
spec:
  schedule:
    cron: "0 2 * * *"
```

#### Run weekdays during business hours

```yaml
spec:
  schedule:
    cron: "0 9-17 * * 1-5"
```

Every hour from 9 AM to 5 PM, Monday through Friday.

### What gets rejected

| `spec.schedule` value | Result | Why |
|---|---|---|
| Both `cron` and `interval` set | `Ready=False / reason=InvalidSpec` with message naming the conflict | FR-005 — exactly one is required |
| Neither set | `Ready=False / reason=InvalidSpec` | Same |
| `cron: "every 6 hours"` | `Ready=False / reason=InvalidSpec` with cron parse error in message | Standard 5-field syntax only |
| `interval: "6 hours"` | `Ready=False / reason=InvalidSpec` | Go-style duration (`"6h"`, not `"6 hours"`) |
| `interval: "0s"` or `"-1h"` | `Ready=False / reason=InvalidSpec` | Must be positive |
| `interval: "500ms"` | `Ready=False / reason=InvalidSpec` | Minimum interval is 1 minute |

### Jitter (no admin action needed)

To prevent 100 CRs from all firing at the same minute boundary, the operator
adds a deterministic per-CR offset (0–59 seconds) hashed from the CR's UID. The
offset is stable across operator restarts — the same CR always fires at the
same offset.

Practical effect: if you `kubectl get namespacescan -A | grep
'nextScheduledScanAt'`, you'll see times spread across the schedule's
granularity rather than all clumped at exact tick boundaries.

### Editing the schedule

Edit `spec.schedule` directly with `kubectl edit`, `kubectl patch`, or by
re-applying the manifest. The new schedule takes effect on the next reconcile
(typically within 1 minute):

```sh
kubectl patch namespacescan scan-prod -n kusari-operator \
  --type merge -p '{"spec":{"schedule":{"interval":"15m"}}}'

# Wait for status.nextScheduledScanAt to update:
kubectl get namespacescan scan-prod -n kusari-operator \
  -o jsonpath='{.status.nextScheduledScanAt}'
```

The next scan fires at `lastScanCompletedAt + 15m` (not 1h, even if the old
schedule said `1h`).

### Forcing a re-scan now

There's no `scan-now` action in v0.9. To trigger an immediate re-scan:

```sh
# Option 1: edit the schedule to a near-future time
kubectl patch namespacescan scan-prod -n kusari-operator \
  --type merge -p '{"spec":{"schedule":{"interval":"1m"}}}'

# Option 2: delete the completed Jobs manually (operator will respawn on next
#           reconcile per feature 007's idempotency)
kubectl delete jobs -n kusari-operator \
  -l kusari.dev/namespace-scan=scan-prod \
  --field-selector status.successful=1
```

A future feature may add a dedicated trigger annotation
(`kusari.dev/trigger-scan-at: <timestamp>`).

## Contributor: extending the scheduler

The scheduler lives at `crates/operator/src/reconcile/scheduler.rs`. Its
public surface is five functions:

```rust
pub fn parse_schedule(spec: &crds::Schedule) -> Result<ScheduleSpec, ScheduleError>;
pub fn compute_next_scheduled_time(schedule: &ScheduleSpec, anchor: DateTime<Utc>, jitter: Duration) -> DateTime<Utc>;
pub fn is_schedule_due(next_scheduled: DateTime<Utc>, now: DateTime<Utc>) -> bool;
pub fn cr_uid_jitter_seconds(uid: &str) -> u64;
pub async fn cleanup_terminal_jobs(api: &Api<Job>, owned: &[Job]) -> Result<usize, kube::Error>;
```

See [contracts/scheduler.md](./contracts/scheduler.md) for invariants.

### Adding timezone support

`spec.schedule.timezone: Option<String>` (e.g., `"America/New_York"`).

1. Add the field to `crate::crds::namespace_scan::Schedule`.
2. Extend `ScheduleSpec::Cron(cron::Schedule)` to carry a `chrono_tz::Tz`.
3. In `compute_next_scheduled_time`, convert the anchor to the target TZ
   before calling `Schedule::after`, then convert back to UTC.
4. Add `chrono-tz` as a workspace dep.
5. Document timezone strings in this quickstart.

### Adding a "scan now" trigger

Two reasonable shapes:

**a)** Annotation on the CR: `kusari.dev/trigger-scan-at: <RFC 3339>`. The
reconciler reads it, compares against `lastScanCompletedAt`, treats it as an
override of `compute_next_scheduled_time` if newer.

**b)** A subresource (`/scan`) on the CRD. Heavier (subresource scaffolding),
but more discoverable via `kubectl explain`.

Recommend (a) for v0.10 — single field add, no schema surgery.

### Adding scan history

`status.scheduleHistory: Vec<{ at: String, outcome: String }>`. Capped at last
N entries. Adds a CRD field, but constitution IV compatible (additive).

## Running the gated E2E locally

```sh
kind create cluster --config e2e/kind-cluster.yaml
docker build -t mikebom-operator:dev .
kind load docker-image mikebom-operator:dev --name mikebom-operator-e2e

MIKEBOM_OPERATOR_E2E=1 cargo test --test schedule_honoring
```

Three test scenarios (each ~10-15s):
- **t_cron_rescan**: tight `cron: "*/2 * * * *"` triggers re-scan within 30s of the next 2-minute tick.
- **t_interval_rescan**: `interval: "2m"` triggers re-scan within 30s of `lastScanCompletedAt + 2m`.
- **t_restart_catchup**: operator restart after 3 missed windows → exactly 1 catch-up scan.
