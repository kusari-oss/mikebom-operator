# Contract: `scheduler` module

Internal contract for the four new functions feature 009 introduces. Not part
of the `operator` crate's public surface, but pinning the boundary keeps
future schedule-related work (manual triggers, timezone support) extending
cleanly.

## `parse_schedule`

### Signature

```rust
pub fn parse_schedule(spec: &crds::Schedule) -> Result<ScheduleSpec, ScheduleError>;
```

### Inputs

- **`spec`**: the CR's `spec.schedule` block. Either `cron: Some(...)`,
  `interval: Some(...)`, both, or neither.

### Outputs

- **`Ok(ScheduleSpec::Cron(s))`** when only `cron` is set AND parses. `s` is a
  `cron::Schedule` ready for `.upcoming(Utc).next()`.
- **`Ok(ScheduleSpec::Interval(d))`** when only `interval` is set, parses as
  a Go-style duration, AND `d >= 60s`.
- **`Err(BothSet)`** when both fields are `Some`.
- **`Err(NeitherSet)`** when both fields are `None`.
- **`Err(InvalidCron(msg))`** when `cron` is set but the cron crate rejects
  it. `msg` is the cron error's `Display`.
- **`Err(InvalidInterval(msg))`** when `interval` is set but `humantime`
  rejects it OR the duration is zero/negative.
- **`Err(IntervalBelowMinimum(d))`** when `interval` parses as `< 60s`.

### Invariants

1. **Total**: every `(cron, interval)` combination produces a deterministic
   `Result`. No panics.
2. **Pure**: no I/O, no system clock reads.
3. **Reused**: callable from both `desired_status` (for InvalidSpec gating)
   and the reconcile schedule-decision path. One source of truth.

## `compute_next_scheduled_time`

### Signature

```rust
pub fn compute_next_scheduled_time(
    schedule: &ScheduleSpec,
    anchor: DateTime<Utc>,
    jitter: Duration,
) -> DateTime<Utc>;
```

### Inputs

- **`schedule`**: from `parse_schedule`.
- **`anchor`**: the schedule's reference time. Caller resolves to
  `status.lastScanCompletedAt`-parsed-as-RFC3339, falling back to
  `metadata.creationTimestamp` if `lastScanCompletedAt` is None. Anchor is
  always a real `DateTime<Utc>` — the contract doesn't accept `Option`.
- **`jitter`**: per-CR jitter from `cr_uid_jitter_seconds`. Added to the
  schedule-computed instant.

### Outputs

The next scheduled scan instant (after `anchor` + `jitter`).

- For `Cron(s)`: `s.after(&anchor).next().expect(...) + jitter`. The cron
  crate's `after` iterator is guaranteed to return at least one upcoming time
  for any valid expression (5-field cron always has infinite future ticks).
- For `Interval(d)`: `anchor + d + jitter`.

### Invariants

1. **Pure**: no I/O, no system clock reads.
2. **Monotonic-ish**: returned time is always > `anchor`. (The jitter
   addition preserves strict monotonicity since jitter is >= 0 seconds
   and the cron-next or interval addition is > 0.)
3. **Deterministic for fixed inputs**: same `(schedule, anchor, jitter)` →
   same output across operator restarts.

## `is_schedule_due`

### Signature

```rust
pub fn is_schedule_due(next_scheduled: DateTime<Utc>, now: DateTime<Utc>) -> bool;
```

### Behavior

```rust
now >= next_scheduled
```

A pure comparison.

### Invariants

1. **Pure**: no I/O.
2. **Monotonic**: if `is_schedule_due(t, now)` is `true`, then it's `true`
   for all `now' >= now`.

## `cr_uid_jitter_seconds`

### Signature

```rust
pub fn cr_uid_jitter_seconds(uid: &str) -> u64;
```

### Behavior

Hashes the CR's `metadata.uid` (a UUIDv4 string from the apiserver) and
returns a value in `0..60`. Implementation: take the first 2 bytes of
`sha2::Sha256::digest(uid.as_bytes())`, interpret as `u16`, take modulo 60.

### Invariants

1. **Pure**: same input → same output forever.
2. **Bounded**: result is always in `[0, 60)`.
3. **Distribution**: roughly uniform across CRs (SHA-256's avalanche
   property gives uniform bucketing for unrelated UID strings).

## `cleanup_terminal_jobs`

### Signature

```rust
pub async fn cleanup_terminal_jobs(
    api: &Api<Job>,
    owned: &[Job],
) -> Result<usize, kube::Error>;
```

### Behavior

For each Job in `owned`:
- If `is_job_succeeded(job)` OR `is_job_finally_failed(job)`:
  - `api.delete(job.metadata.name, &DeleteParams { propagation_policy: Some(PropagationPolicy::Foreground), .. }).await`
  - On `kube::Error::Api(e) if e.code == 404`: treat as success (Job
    already gone — possibly via `ttlSecondsAfterFinished` race).
  - Other errors propagate.
- Otherwise: skip. The Job is in progress (FR-011).

Returns the count of Jobs that were actually deleted (excludes 404s and
skips).

### Invariants

1. **Filter correctness** (FR-011): in-progress Jobs (succeeded < 1 AND not
   finally failed) are NEVER deleted by this function.
2. **Idempotent**: re-running against the same input is a no-op after the
   first call (deleted Jobs surface as 404 = success).
3. **Atomic per-Job**: deletion failures on one Job halt the iteration. The
   caller's reconcile retry handles partial-progress.
4. **No SBOM access**: contract only reads `job.status` and
   `job.metadata.name`.

## Non-goals (out of scope for feature 009)

- **Manual trigger API** ("scan now" annotation, CR field, or k8s
  subresource). Admins force re-scan by editing any field on the CR.
- **Timezone-aware cron**. Cron is UTC-only in v0.9; a future feature can add
  `spec.schedule.timezone`.
- **Concurrent scan queueing**. If a scan takes longer than the interval, the
  cadence becomes "back-to-back" with no queue. Warning conditions are
  deferred.
- **Backfill of missed scans**. FR-008 mandates exactly ONE catch-up after
  restart, not N.
- **Manual jitter override** (per-CR `spec.schedule.jitter`). Jitter is
  deterministic from the UID; admins don't tune it.
- **Schedule history** (`status.scheduleHistory[]`). The single
  `lastScanCompletedAt` + `nextScheduledScanAt` are the only schedule-visible
  state.
