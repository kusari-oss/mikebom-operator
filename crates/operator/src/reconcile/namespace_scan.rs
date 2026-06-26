//! Main reconciler for `NamespaceScan` CRs.
//!
//! Feature 002 (per plan §10) implements the controller loop:
//! list pods in target namespaces → diff against `status.scannedImages` →
//! spawn a 3-container Job per new image → update status on completion.
