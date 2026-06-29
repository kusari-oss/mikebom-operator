# Feature Specification: Status feedback from Job watch

**Feature Branch**: `008-job-status-feedback`

**Created**: 2026-06-28

**Status**: Draft

**Input**: User description: "status feedback from Job watch"

## User Scenarios & Testing *(mandatory)*

### User Story 1 — Admin sees scan completion (Priority: P1)

A cluster admin applies a `NamespaceScan` CR. Feature 007 spawns one scan Job per distinct in-scope image; the CR's status transitions to `Ready=False / reason=Scanning`. As the Jobs run to completion and produce SBOMs, the admin watches the CR's status update to reflect the final outcome — `Ready=True / reason=ScanCompleted` once every Job finishes successfully — without manually inspecting Job objects.

**Why this priority**: This is the moment the operator stops *lying*. Up through feature 007, a CR sits at `reason=Scanning` indefinitely even when the underlying Jobs finished an hour ago. P1 because (a) the only meaningful "did my SBOM generation actually work?" signal an admin gets is on the CR, and (b) downstream tooling (GitOps health checks, dashboards, kubectl wait) keys off `Ready=True` — without it, the operator is invisible to standard k8s ergonomics.

**Independent Test**: With Phase 3 landed: apply a CR whose target is a single pod running `nginx:1.27.0`. Wait for the scan Job to complete (mikebom-scan succeeds, output-upload succeeds). Within 5 seconds of the Job's `status.succeeded=1` transition, `kubectl get namespacescan <name> -o json | jq '.status.conditions[] | select(.type=="Ready") | .status, .reason'` returns `"True"` and `"ScanCompleted"`.

**Acceptance Scenarios**:

1. **Given** a CR with one in-scope image whose scan Job has just transitioned to `status.succeeded=1`, **When** the operator receives the Job-watch event, **Then** within 5 seconds the CR's `status.conditions[type=Ready]` reports `status=True, reason=ScanCompleted`.
2. **Given** a CR with three in-scope images and three running scan Jobs, **When** two Jobs have completed successfully and one is still running, **Then** the CR's status stays at `Ready=False / reason=Scanning` (mixed-completion is not yet `ScanCompleted`).
3. **Given** a CR whose three scan Jobs all succeed in close succession, **When** the operator processes the watch events, **Then** the final reported state is `Ready=True / reason=ScanCompleted` (intermediate `Scanning` states between events are acceptable as long as the final state is correct).
4. **Given** a CR at `Ready=True / reason=ScanCompleted`, **When** the operator's periodic 5-minute requeue fires and no Jobs have changed, **Then** the CR's status remains `Ready=True / reason=ScanCompleted` and `lastTransitionTime` does not advance.

---

### User Story 2 — Admin sees scan failure (Priority: P1)

A scan Job exhausts its retry budget (`backoffLimit + 1` failed pods) due to a permanent error — broken image reference, network failure pulling the image, mikebom-scan crash, output-upload credentials wrong. The admin sees `Ready=False / reason=ScanFailed` on the CR, with a message naming which image's scan failed and why.

**Why this priority**: Without this, a scan failure is silent — the CR shows `Scanning` forever and the only way to find out is `kubectl get jobs` plus log-spelunking. P1 (tied with US1) because successful scans without failure visibility are an incomplete user model: the admin can't trust `ScanCompleted` if `ScanFailed` doesn't exist as the complementary signal.

**Independent Test**: Apply a CR whose target is a single pod running an image the operator can't pull (`registry.invalid/does-not-exist:never`). Within ~5 minutes (Job retry × `backoffLimit`), `kubectl get namespacescan <name> -o json` returns `Ready=False / reason=ScanFailed`, with a `message` naming `registry.invalid/does-not-exist:never`.

**Acceptance Scenarios**:

1. **Given** a CR with one in-scope image whose scan Job has transitioned to `status.failed >= backoffLimit + 1`, **When** the operator processes the watch event, **Then** within 5 seconds the CR's `status.conditions[type=Ready]` reports `status=False, reason=ScanFailed`, and the message names the failing image.
2. **Given** a CR with three in-scope images: 2 Jobs succeeded, 1 Job exhausted retries, **When** the operator processes the failure event, **Then** the CR's final reason is `ScanFailed` (failure dominates partial-success).
3. **Given** a CR at `ScanFailed` for one image, **When** the admin patches the failing pod to use a valid image, **Then** on the next reconcile the operator spawns a new Job for the new image (per feature 007's image dedup), and the CR transitions back to `Scanning` while the new Job runs.

---

### User Story 3 — Admin sees per-image scan records (Priority: P2)

After a successful scan completes, the CR's `status.scannedImages[]` array records one entry per scanned image: the image reference, when scanning completed, and where the SBOM ended up (PVC path, S3 object key, or OCI artifact ref). The admin can run `kubectl get namespacescan -o json | jq '.status.scannedImages[]'` to enumerate "what got scanned, and where the SBOMs are."

**Why this priority**: The CR's condition gives a single bit ("done" vs "scanning"). `scannedImages[]` gives the *manifest* — what's the inventory of SBOMs we produced, and where can downstream tooling find them? P2 because US1 + US2 deliver the immediate "is it done?" signal; this story delivers the data needed to actually go *use* the SBOMs. Admins can ship without it (find SBOMs via the output backend directly) but observability is degraded.

**Independent Test**: Apply a CR with `output.type=S3, output.s3.bucket=test-sboms`. After scans complete: `kubectl get namespacescan <name> -o json | jq '.status.scannedImages[]'` returns one entry per distinct in-scope image, each with `imageRef`, `completedAt` (RFC 3339), and `sbomLocation` (an `s3://test-sboms/<short-hash>.json` URL).

**Acceptance Scenarios**:

1. **Given** a CR with PVC output backend and two in-scope images whose Jobs both succeed, **When** the operator processes both completion events, **Then** `status.scannedImages[]` has exactly two entries, each with `sbomLocation` formatted as `pvc://<claimName>/<pathPrefix>/<short-hash>.<ext>`.
2. **Given** a CR with S3 output backend and image `nginx:1.27.0`, **When** the Job for that image succeeds, **Then** the corresponding `scannedImages[]` entry has `imageRef="nginx:1.27.0"` and `sbomLocation` formatted as `s3://<bucket>/<pathPrefix>/<short-hash>.<ext>`.
3. **Given** a CR with OCI output backend, **When** Jobs succeed, **Then** each `scannedImages[]` entry's `sbomLocation` is formatted as `oci://<registry>/<repository>:<short-hash>`.
4. **Given** a CR whose image set changes between reconciles (one pod's image is updated), **When** the new image's Job completes, **Then** the previous image's `scannedImages[]` entry remains (not removed) and a new entry for the new image appears.

---

### Edge Cases

- **Job manually deleted by the admin while running**: the watch event fires with no observable `status.succeeded` or `status.failed`. The operator MUST NOT treat this as `ScanFailed` — feature 007's `ensure_jobs` will re-create the Job on the next reconcile (deterministic naming + idempotent create). The CR's status stays at `Scanning` (the new Job is in scope).
- **Job exists but its pod was OOMKilled and the Job's `status.failed` count incremented but is still below `backoffLimit`**: this is a retry-in-progress, not a final failure. The CR's status stays at `Scanning` until either `status.succeeded=1` (success) or `status.failed >= backoffLimit + 1` (exhausted retries → `ScanFailed`).
- **Job completed before the operator started watching** (operator restart, lost watch resync): the operator MUST observe the existing Job's terminal state via the initial watcher list and update status accordingly on the first reconcile after restart. No "stuck Scanning" state across operator restarts.
- **Job has `status.completionTime` set but `status.succeeded` is unexpectedly missing/zero** (k8s field semantics edge case): treat as success only if `status.succeeded >= 1`; otherwise treat as still running. The Kubernetes API guarantees `succeeded` is incremented before `completionTime` in well-behaved controllers.
- **CR `output` backend edited mid-scan**: the spawned Jobs were snapshotted with the old backend (feature 007 spec edge case). `sbomLocation` strings written to `scannedImages[]` MUST reflect the *Job's* output backend at spawn time, not the CR's current `output` field. (Source of truth: the Job's labels / spec, not the CR.)
- **CR at `ScanFailed` and admin re-applies the same CR (no spec change)**: the failed Job(s) still exist with `status.failed >= backoffLimit + 1`. Feature 007's `ensure_jobs` does NOT respawn (Acceptance Scenario 3.2 in feature 007). The CR's status stays at `ScanFailed` until the admin manually deletes the failed Jobs or edits the CR to fix the underlying issue.
- **Multiple CRs target overlapping namespaces sharing an image**: each CR owns its own Job (feature 007 invariant — per-CR dedup). Each CR's `scannedImages[]` is populated independently; one CR's `ScanCompleted` does not affect another's status.
- **Job watch event received for a Job whose `ownerReferences` doesn't point at any current `NamespaceScan`**: the operator MUST ignore it (likely a stale Job from a deleted CR). Garbage collection handles the cleanup.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The operator MUST subscribe to a Kubernetes watch on `batch/v1.Job` objects in the operator's own namespace and enqueue the owning `NamespaceScan` CR for reconcile whenever a watched Job's `status` field transitions.
- **FR-002**: On each reconcile, after feature 007's `ensure_jobs` returns `OrchestrationResult::Spawned`, the operator MUST list all Jobs owned by the CR (via the `kusari.dev/namespace-scan=<cr-name>` label) and inspect their `.status` fields to determine the aggregate scan outcome.
- **FR-003**: When every Job owned by a CR has `.status.succeeded >= 1`, the operator MUST set the CR's `status.conditions[type=Ready]` to `status=True, reason=ScanCompleted` with a message naming the count of scanned images.
- **FR-004**: When any Job owned by a CR has `.status.failed >= .spec.backoffLimit + 1` (the Kubernetes definition of "exhausted retries"), the operator MUST set `Ready=False, reason=ScanFailed` with a message naming the failing image reference. If multiple Jobs failed, naming any one of them satisfies this requirement.
- **FR-005**: When at least one owned Job is neither finally succeeded nor finally failed (i.e., still running or retrying within `backoffLimit`), the operator MUST keep `Ready=False, reason=Scanning`. Failure overrides partial-success in the aggregation order: `any failed → ScanFailed`; otherwise `any still running → Scanning`; otherwise `all succeeded → ScanCompleted`.
- **FR-006**: For every Job that has transitioned to `.status.succeeded >= 1`, the operator MUST append (or update if already present) one entry to the CR's `status.scannedImages[]`. Each entry MUST populate `imageRef`, `completedAt` (= the Job's `.status.completionTime`, formatted as RFC 3339), and `sbomLocation` (a backend-specific URL — see FR-008).
- **FR-007**: The `imageRef` written to `scannedImages[]` MUST be reconstructible from the Job (e.g., from a label set by feature 003's builder, or from the Job's container env). The operator MUST NOT re-resolve the image by re-querying target pods at status-update time, since the pod set may have changed since the Job was spawned.
- **FR-008**: The `sbomLocation` string MUST reflect the *Job's* output backend at spawn time, not the CR's current `output` field. The format MUST be one of:
  - `pvc://<claimName>/<pathPrefix>/<short-hash>.<ext>` for PVC output
  - `s3://<bucket>/<pathPrefix>/<short-hash>.<ext>` for S3 output
  - `oci://<registry>/<repository>:<short-hash>` for OCI output
- **FR-009**: When a CR's `OrchestrationResult` is `NoImagesInScope`, `BuildFailed`, or `RbacInsufficient` (feature 007 reasons), this feature MUST NOT override the status. Job-status aggregation only runs when feature 007 returned `Spawned`.
- **FR-010**: After an operator restart, the watch MUST resync from the apiserver. On the first reconcile after restart, the operator MUST observe any pre-existing Jobs' terminal states and update status accordingly without requiring the Jobs to re-transition.
- **FR-011**: The operator MUST ignore watch events for Jobs whose `ownerReferences` does not reference any currently-extant `NamespaceScan` CR. The operator MUST NOT error on such events; they MUST be logged at debug level and discarded.
- **FR-012**: Feature 007's `Scanning` / `NoImagesInScope` / `BuildFailed` / `RBACInsufficient` reasons MUST continue to work unchanged. Feature 007's E2E tests MUST continue to pass without modification.
- **FR-013**: Feature 002's `InvalidSpec` short-circuit MUST continue to work unchanged. Feature 008 does not consider Job status for invalid specs.
- **FR-014**: The operator MUST NOT actively delete completed Jobs as part of this feature. Job cleanup is governed by the existing `ttlSecondsAfterFinished` (set by feature 003) and Kubernetes garbage collection on CR delete (feature 007's owner refs). Re-scanning is out of scope and lands with schedule-honoring.
- **FR-015**: `scannedImages[]` MUST be append-only within the lifetime of a single CR. When a previously-scanned image is no longer in the target set (the underlying pod was removed or its image changed), the operator MUST leave the old `scannedImages[]` entry in place. Pruning is deferred to a future feature.

### Key Entities

- **Job watch subscription (new)**: a Kubernetes-watch source the operator subscribes to. Filters to Jobs labeled `kusari.dev/namespace-scan=<any>` in the operator's namespace.
- **ScannedImage record (existing, now populated)**: the `status.scannedImages[]` element type from the v1alpha1 CRD (feature 001). Fields: `imageRef`, `resolvedSha` (left `None` in v0.8 — see Assumptions), `sbomLocation`, `completedAt`. Feature 008 populates 3 of 4 fields per record.
- **Owned Job set (per-CR)**: the set of Jobs whose ownership label matches the CR's name. Used as the unit of aggregation for the FR-005 decision table.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: With one CR targeting 3 distinct images that all scan successfully, the CR's status transitions to `Ready=True / reason=ScanCompleted` within 5 seconds of the final Job's `status.succeeded=1` transition. (Verifies FR-001, FR-002, FR-003.)
- **SC-002**: With one CR targeting one image whose Job exhausts retries (`backoffLimit=2`, three failed pods), the CR's status transitions to `Ready=False / reason=ScanFailed` within 5 seconds of the final retry exhaustion. (Verifies FR-001, FR-004.)
- **SC-003**: With a CR targeting 3 images and Jobs in mixed state (1 succeeded, 1 running, 1 failed), the CR's reason is `ScanFailed` (failure dominates). The mixed-state transition does NOT pass through `ScanCompleted` even momentarily. (Verifies FR-005.)
- **SC-004**: After 3 successful scans against a CR with PVC output backend, `status.scannedImages[]` has 3 entries, each with `imageRef` populated, `completedAt` as an RFC 3339 string, and `sbomLocation` matching the pattern `pvc://<claim>/<prefix>/<7-char-hex>.json`. (Verifies FR-006, FR-007, FR-008 — PVC variant.)
- **SC-005**: After a CR at `ScanCompleted` has its `output` field edited and a new pod is added (triggering a new Job spawn), the previously-recorded `scannedImages[]` entries are preserved (append-only), and a new entry appears once the new Job completes. (Verifies FR-015.)
- **SC-006**: After an operator pod restart, a CR whose Jobs completed during the restart window correctly reports `Ready=True / reason=ScanCompleted` within 30 seconds of the operator's recovery. (Verifies FR-010.)

## Assumptions

- **Resolved digest is deferred**: `ScannedImage.resolvedSha` stays `None` in v0.8. Capturing the runtime-resolved image SHA requires either reading the target pod's `status.containerStatuses[].imageID` (which conflates "what's running now" with "what was scanned") or instrumenting the Job's init-pull container to surface the digest. Both options have v0.x-scope-creep risk; deferred to a follow-up.
- **`backoffLimit` retry semantics**: the spec relies on Kubernetes's documented semantics — a Job is "finally failed" when `.status.failed >= .spec.backoffLimit + 1`. The operator does not introduce its own retry budget on top.
- **Watch lag is bounded**: kube-rs's `Controller::watches` typically delivers events within a few hundred milliseconds. The 5-second budget in SC-001/SC-002 includes the watch event + reconcile + status patch round-trip plus headroom.
- **`scannedImages[]` size is bounded by the unique-image set per CR**: in v0.7's scale assumption (≤25 distinct images per CR), `scannedImages[]` stays under 25 entries. No paging or compaction needed.
- **SBOM location strings are derived, not recorded by the Job**: the operator computes the location from the Job's spec/labels + the CR's `output` block (at spawn time). The Job itself doesn't write back a "here's where I uploaded" status — that would require coupling between mikebom-scan + the output-upload container + the operator, which conflicts with constitution II's USE-not-EMBED stance.
- **No retroactive re-scan on failure remediation**: if a Job fails permanently and the admin fixes the CR/cluster, feature 007's `ensure_jobs` will NOT respawn the failed Job (Acceptance Scenario 3.2 in feature 007). The admin must either delete the failed Job manually or wait for a future feature to add an explicit "retry" path.
- **Job labels are the source of truth for image-ref lookup**: feature 003's builder already sets `kusari.dev/namespace-scan=<cr-name>` and `kusari.dev/image-ref-hash=<7-char-hex>` on every Job. v0.8 may need an additional label `kusari.dev/image-ref=<image-ref>` if the literal image string is needed for `scannedImages[]`. Adding a label is constitution-IV-compatible (additive). Alternative: read the image from the Job's `init-pull` container's `IMAGE_REF` env var.
- **Constitution VI E2E**: a new kind-based E2E asserts the end-to-end "CR applied → Jobs run → status transitions to ScanCompleted / ScanFailed → scannedImages populated" path. This requires running the actual operator pod (chart install + image load), not just the in-process orchestrator — because watch-based reconciliation is a *runtime* behavior that doesn't manifest by calling `reconcile()` once.
- **Chart RBAC**: no update needed. The existing ClusterRole already grants `batch/v1.jobs:get,list,watch,create,delete` cluster-wide (verified during feature 007 planning).
- **CRD shape stays additive**: no new fields are required by this feature; `status.scannedImages[]` and `status.lastScanCompletedAt` already exist in the v1alpha1 schema (feature 001/002). This feature only populates them. `lastScanCompletedAt` is populated to the most recent `completedAt` across all `scannedImages[]` entries.
