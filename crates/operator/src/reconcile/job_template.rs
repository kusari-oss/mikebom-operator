//! Builds the 3-container `batch/v1` Job per plan §3 (Pre-condition).
//!
//! Containers (in order, sharing an `emptyDir` mounted at `/workdir`):
//!   1. `init-pull`     — `skopeo copy` + layer extract → `/workdir/rootfs`
//!   2. `mikebom-scan`  — `mikebom sbom scan --path /workdir/rootfs ...`
//!   3. `output-upload` — pushes the SBOM artifact to the configured backend.
//!
//! Feature 003 (per plan §10) implements the spec builder.
