# Feature Specification: Reconciler spawns scan Jobs

**Feature Branch**: `007-reconciler-spawns-job`

**Created**: 2026-06-28

**Status**: Draft

**Input**: User description: "reconciler spawns scan"

## Clarifications

### Session 2026-06-28

- Q: Which pod phases contribute their images to the dedup set? → A: Only pods in phase `Running` or `Pending` are in scope. Pods in `Succeeded`, `Failed`, or `Unknown` are excluded; their images do not produce scan Jobs.

## User Scenarios & Testing *(mandatory)*

### User Story 1 — Operator spawns a scan Job per target image (Priority: P1)

A cluster admin applies a `NamespaceScan` CR whose `target.namespaces` lists one or more namespaces containing running pods. The operator enumerates the workloads in those namespaces, collects the set of distinct container images, and for each image creates a `batch/v1.Job` that runs mikebom against that image. The admin can verify by listing Jobs in the operator's namespace and seeing one per distinct image.

**Why this priority**: This is the end-to-end "the operator scans things" moment. Up to feature 006 the operator only acknowledged CRs (feature 002) and had a builder that produced Job manifests (features 003–006) but never created them. P1 because it converts six features of dormant code into observable behavior — without it, the operator delivers no scanning value.

**Independent Test**: Apply a CR with `target.namespaces: [default]` while the `default` namespace runs pods using `nginx:1.27.0` and `redis:7.4.0`. Within 30 seconds of applying the CR, exactly two `batch/v1.Jobs` exist in the operator's namespace — one per distinct image — each owned by the `NamespaceScan` CR. Demonstrable end-to-end against a kind cluster.

**Acceptance Scenarios**:

1. **Given** a `NamespaceScan` CR targeting a namespace with pods running images `A` and `B`, **When** the operator reconciles the CR, **Then** exactly one Job is created per distinct image (2 Jobs total).
2. **Given** a `NamespaceScan` CR targeting a namespace with three pods all running image `A`, **When** the operator reconciles, **Then** exactly one Job for image `A` is created (image-level deduplication, not pod-level).
3. **Given** a `NamespaceScan` CR with `target.namespaces: []` and no `target.labelSelector`, **When** the operator reconciles, **Then** the CR's status condition reflects `InvalidSpec` (per feature 002) and no Jobs are created.
4. **Given** a CR whose `target` matches a real namespace that currently has zero pods, **When** the operator reconciles, **Then** no Jobs are created, no errors are logged, and the CR's status reflects a "nothing to scan yet" state distinguishable from `Scanning`.

---

### User Story 2 — Status condition reflects scan in progress (Priority: P2)

After the operator spawns one or more Jobs for a CR, the CR's `status.conditions[Ready]` updates to communicate that scanning is in progress. The admin observes this transition via `kubectl get namespacescan`.

**Why this priority**: Feature 002 reserved the `Scanning` reason in `docs/architecture.md`. Until something actually transitions to it, the status surface lies: every valid CR sits at `NotYetReconciled` even when the operator is actively working. P2 because the operator works without it (Jobs still run, SBOMs still land) but the user-facing signal is misleading.

**Independent Test**: Apply a CR that produces at least one Job per US1. Within 10 seconds, `kubectl get namespacescan <name> -o json | jq '.status.conditions[] | select(.type=="Ready").reason'` returns `Scanning` (not `NotYetReconciled`).

**Acceptance Scenarios**:

1. **Given** a valid `NamespaceScan` CR with at least one image in scope and an operator that has spawned the corresponding Jobs, **When** the admin reads the CR's status, **Then** `status.conditions[type=Ready]` has `status=False` and `reason=Scanning`.
2. **Given** a valid CR for which the operator has spawned Jobs in a prior cycle, **When** a subsequent reconcile runs (Jobs still exist, still owned by the CR), **Then** the status reason remains `Scanning` and `lastReconciledAt` advances.
3. **Given** a CR whose target resolves to zero pods, **When** the operator reconciles, **Then** the status reason is `NoImagesInScope` (or equivalent non-error, non-`Scanning` reason) so the admin can distinguish "nothing to scan" from "actively scanning."

---

### User Story 3 — Idempotent Job creation across reconciles (Priority: P2)

The reconciler's requeue interval, edits to the CR, or operator restarts cause `reconcile()` to run multiple times against the same `(CR, image)` pair. The operator MUST NOT create a duplicate `Job` each time — at most one live `Job` per `(CR, image)` exists at any moment.

**Why this priority**: Without this, every requeue (currently 5 minutes for valid CRs) would queue another scan of the same image, blowing up the cluster's Job count and the registry pull bill. P2 (tied with US2) because it's a correctness property the operator can't ship without, but US1 must land first to have anything to deduplicate.

**Independent Test**: Apply a CR. Wait for the first reconcile to spawn Jobs. Trigger a forced re-reconcile (e.g., edit a benign field on the CR). Confirm the Job count for that `(CR, image)` did not increase.

**Acceptance Scenarios**:

1. **Given** an operator that has already spawned a Job for `(CR=scan-prod, image=nginx:1.27.0)`, **When** the operator reconciles `scan-prod` again with the same target pods, **Then** no new Job is created for that `(CR, image)` and the existing Job is left running.
2. **Given** a Job from a prior reconcile has completed (success or failure) and still exists in the cluster, **When** the operator reconciles the same CR with the same target image, **Then** the operator does not create a second Job for that `(CR, image)` in the same reconcile cycle. (A separate "re-scan on completion" loop is out of scope for v0.7; see Assumptions.)
3. **Given** the target namespace's pods change between reconciles to run a *new* image not previously scanned, **When** the operator reconciles, **Then** a new Job is created for the new image while the existing Job for the old image is untouched.

---

### Edge Cases

- **Pods restarting / in `ContainerCreating` mid-reconcile**: the pod is in phase `Pending`, so it is in scope per FR-001. The operator reads `pod.spec.containers[].image` (the requested image, not the resolved digest from `status`), so a pod that hasn't pulled yet still contributes its image. Resolved-SHA capture moves to feature 008.
- **Pods in phase `Succeeded` or `Failed`** (e.g., the pod of a completed Kubernetes Job that the cluster operator hasn't cleaned up): excluded by FR-001. Their images do not produce scan Jobs even when no other pod in the namespace runs the same image. This avoids SBOMs that reflect past, not present, workloads.
- **InitContainers and ephemeralContainers**: in scope. Their images count as distinct entries for the dedup set, so an init image and a runtime image of the same pod produce two Jobs.
- **Pod uses an image with no tag and no digest** (e.g., `nginx`): the operator MUST treat the unqualified reference as the deduplication key; it MUST NOT silently inject `:latest` or pull-resolve the digest. (Pull-time resolution happens inside the scan Job, not the reconciler.)
- **`target.kinds` includes non-Pod kinds (Deployment, StatefulSet, etc.)**: out of scope for feature 007. The operator MUST treat `kinds: [Pod]` (the documented default) as the only honored value and ignore other entries silently. A future feature widens this; the spec stays additive.
- **Operator RBAC lacks `batch/v1.jobs:create`** or `pods:list` in a target namespace: the operator MUST surface `Ready=False` with `reason=RBACInsufficient` (constitution III) and MUST NOT silently skip Job creation. The message MUST name the missing verb and resource.
- **Target namespace does not exist**: this is a transient condition (the user may create it). The operator surfaces `reason=NoImagesInScope` and requeues at the normal cadence; it does not error.
- **Two CRs target overlapping namespaces / images**: each CR owns its own Jobs (deduplication is per-CR). A `(CR-A, image-X)` Job and a `(CR-B, image-X)` Job coexist. This is intentional — different CRs may have different output backends.
- **CR is deleted while a spawned Job is running**: the Job's `metadata.ownerReferences` points at the CR, so Kubernetes garbage-collects the Job (and its pod) automatically. The operator does not need a finalizer.
- **CR edits the `output` backend after Jobs have spawned**: the existing Jobs continue with the old backend (they're snapshotted). New Jobs spawned for new images use the new backend. The operator MUST NOT delete and re-create live Jobs in response to an `output` field edit.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: For every `NamespaceScan` CR with a valid spec (per feature 002's `desired_status()`), the operator MUST enumerate pods in the resolved target namespaces whose `status.phase` is `Running` or `Pending`, and collect the set of distinct container image references (treating `initContainers`, `containers`, and `ephemeralContainers` as a single set per pod). Pods in phase `Succeeded`, `Failed`, or `Unknown` MUST be excluded from enumeration.
- **FR-002**: For each distinct image reference produced by FR-001 that does not yet have a corresponding live `Job` for this CR, the operator MUST create a `batch/v1.Job` constructed by `crate::scan_job::build_scan_job(...)` (feature 003's pure builder).
- **FR-003**: Every spawned Job MUST be created in the operator's own namespace (not in the target namespace), to bound the RBAC blast radius to a single namespace per cluster.
- **FR-004**: Every spawned Job's `metadata.name` MUST be a deterministic function of `(CR name, image reference)` so the reconciler can perform a get-before-create check using only the CR + image. The naming function MUST be stable across operator restarts.
- **FR-005**: Every spawned Job MUST set `metadata.ownerReferences` to point at the `NamespaceScan` CR with `controller=true` and `blockOwnerDeletion=true`, so Kubernetes garbage-collects the Job when the CR is deleted.
- **FR-006**: When at least one Job spawned by FR-002 exists for a CR, the operator MUST set the CR's `status.conditions[type=Ready]` to `status=False` with `reason=Scanning` and an explanatory message.
- **FR-007**: When a valid CR has zero target images in scope (target namespace exists but is empty, or labelSelector matches no pods), the operator MUST set `reason` to a value distinguishable from both `NotYetReconciled` and `Scanning` (e.g., `NoImagesInScope`).
- **FR-008**: When the operator lacks RBAC to list pods in any target namespace, or lacks RBAC to create Jobs in the operator's own namespace, the operator MUST surface `Ready=False` with `reason=RBACInsufficient` and a message naming the missing verb and resource. The operator MUST NOT silently degrade.
- **FR-009**: Reconcile MUST be idempotent: re-running reconcile against a CR whose Jobs already exist (by FR-004's deterministic naming) MUST NOT create duplicate Jobs and MUST NOT error.
- **FR-010**: The reconciler MUST treat a kube API `409 Conflict` on Job creation (raced create by another reconcile) as a non-error: the conflicting Job is taken to satisfy the get-before-create check for that `(CR, image)` and reconcile proceeds.
- **FR-011**: Reconcile MUST complete its enumeration + Job-creation phase within 30 seconds for a target scope of up to 100 pods spanning up to 25 distinct images (the v0.7 scale assumption).
- **FR-012**: Feature 002's `lastReconciledAt` and `InvalidSpec` behavior MUST continue to work unchanged. Feature 002's E2E tests MUST continue to pass without modification.
- **FR-013**: Features 003–006's `scan_job::build_scan_job` builder and its unit tests MUST continue to work unchanged. The reconciler is a new caller; the builder's signature does not move.
- **FR-014**: The reconciler MUST NOT read or interpret Job status (`.status.succeeded`, `.status.failed`, etc.) in feature 007. Status feedback (`ScanCompleted`, `ScanFailed`) is the next feature's responsibility.
- **FR-015**: The reconciler MUST NOT enforce or honor `spec.schedule` in feature 007. Schedule honoring is a separate feature. The reconciler MAY use its own requeue cadence (currently 5 minutes for valid specs) to retry image enumeration.

### Key Entities

- **NamespaceScan CR (existing)**: the user-visible resource. Owns spawned Jobs via `ownerReferences`. Its `status.conditions[Ready]` reflects scan progress.
- **Scan Job (existing builder, new lifecycle)**: a `batch/v1.Job` constructed by feature 003's builder. Lives in the operator's namespace. Has exactly one `(CR, image)` it scans, encoded in its name and inferable from its `ownerReferences`.
- **Image reference (new dedup key)**: the literal string from `pod.spec.{containers,initContainers,ephemeralContainers}[].image`. Used as the dedup key within a single CR's reconcile cycle.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: With one `NamespaceScan` CR targeting a namespace containing 5 distinct images across 20 pods (all in phase `Running`), the operator spawns exactly 5 Jobs within 30 seconds of CR application. (Verifies FR-001, FR-002, FR-011.)
- **SC-002**: Re-applying the same CR (or letting the 5-minute requeue fire) produces 0 new Jobs when the target image set is unchanged. (Verifies FR-009.)
- **SC-003**: Deleting the CR triggers Kubernetes garbage collection of every spawned Job within the default GC SLA (typically <30s). (Verifies FR-005.)
- **SC-004**: A CR targeting an empty namespace reaches `Ready=False / reason=NoImagesInScope` within 10 seconds and remains stable across requeues. (Verifies FR-007.)
- **SC-005**: A CR for which the operator lacks `pods:list` RBAC in the target namespace surfaces `Ready=False / reason=RBACInsufficient` within 10 seconds, with a message naming `list pods in namespace <X>`. (Verifies FR-008.)
- **SC-006**: Every Job spawned by feature 007 is a syntactically valid `batch/v1.Job` accepted by `kubectl apply --dry-run=server`. (Inherits feature 003's invariant; verified by reusing the dry-run E2E harness.)

## Assumptions

- **Job naming scheme**: deterministic. The planning phase will pick a concrete shape (e.g., `<cr-name>-<image-short-hash>`); the spec only requires that it be a pure function of `(CR, image)`. This avoids the planner being forced to honor an arbitrary spec-time choice.
- **One Job per (CR, image) lifecycle**: once a Job exists, the operator does not create another for the same key, even after the existing Job completes. Re-scan-on-completion (e.g., every Job runs to completion, then a fresh Job replaces it on the next reconcile) is out of scope for v0.7 and lands with the schedule-honoring feature.
- **Pod images, not resolved digests**: the dedup key is `pod.spec.containers[].image` (the requested image string). Resolving to a digest from `pod.status.containerStatuses[].imageID` is deferred to feature 008's status-from-Job work, where the runtime SHA gets recorded on `status.scannedImages[].resolvedSha`.
- **Jobs land in the operator's namespace**: bounding RBAC to one namespace. The operator's chart-installed ServiceAccount needs `batch/v1.jobs:create,get,list,watch,delete` in its own namespace and `pods:list` in target namespaces (cluster-wide for v0.7's simple chart shape; namespace-scoped variants are a chart-level enhancement).
- **`target.kinds` honors only `Pod` in v0.7**: workload kinds (Deployment, StatefulSet, DaemonSet) are not enumerated. The default kind list is `[Pod]` (per CRD reference docs), so this matches existing behavior; explicit kind requests other than `Pod` are silently ignored.
- **No reactive re-scan on workload change**: between reconciles, if the target pods' image set changes, the next reconcile (5 minutes later) picks it up. Watch-based instant reaction to pod changes is a later optimization.
- **Single output backend per CR**: a CR's `output` field is snapshotted into the Job at spawn time. Editing the backend mid-flight only affects future Jobs (per Edge Cases).
- **Constitution VI E2E**: the test reuses the kind-cluster fixture already in `e2e/`. A new E2E asserts the end-to-end "CR applied → Jobs spawned → owner refs set" path; it does not assert SBOM contents (that's feature 008+).
- **Helm chart RBAC update**: NOT required (corrected during planning). A `grep` of `charts/mikebom-operator/templates/rbac.yaml` (recorded in plan.md's Constitution Check, row VII) confirms the existing ClusterRole already grants `pods:get,list,watch` cluster-wide and `jobs:get,list,watch,create,delete` cluster-wide — both verbs feature 007 needs. The CRD shape itself does not change, so the chart YAML drift check (feature 001) stays green; no chart templates change in this feature.
