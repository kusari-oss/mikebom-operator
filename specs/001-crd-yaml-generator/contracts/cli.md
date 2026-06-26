# CLI Contract: `mikebom-operator-ctl crd`

This document defines the v0.1 stability contract for the `crd` subcommand of `mikebom-operator-ctl`.

## Synopsis

```text
mikebom-operator-ctl crd [--output <PATH>]
```

## Behavior

- Emits the `NamespaceScan` `CustomResourceDefinition` as YAML.
- Default destination: standard output.
- With `--output <PATH>`: writes to `PATH`, overwriting any existing file. Standard output is silent in this mode.

## Exit codes

| Code | Meaning |
|------|---------|
| 0 | YAML emitted successfully |
| 1 | I/O error (e.g., `--output PATH` is not writable, parent directory missing) |
| 2 | Argument parse error (clap default) |

## Output format

- UTF-8 YAML, LF line endings.
- Single document (no leading `---`); future CRDs will use a separate invocation or v0.2's positional argument.
- Trailing newline at EOF.
- Deterministic key order: alphabetical inside maps, struct-field declaration order for derived types.
- No comments, no kubectl-style annotations.

## Stability guarantees (v0.1)

- The `crd` subcommand exists with the synopsis above. Adding additional sibling subcommands (`dry-run`, `crds`, etc.) does not break this contract.
- v0.1 has no positional argument. v0.2+ may add an optional positional defaulting to `NamespaceScan`; the no-arg form remains valid forever.
- Output content matches `charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml` byte-for-byte (enforced by the `crd_drift.rs` integration test). Anyone who depends on the YAML bytes via `mikebom-operator-ctl crd | <consumer>` gets the same content as anyone who reads the chart file directly.

## Examples

```sh
# Print the CRD to stdout
mikebom-operator-ctl crd

# Regenerate the chart's CRD file in place
cargo run --bin mikebom-operator-ctl -- crd \
  --output charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml

# Verify the chart YAML matches the generator (cargo test does this too)
diff <(cargo run --bin mikebom-operator-ctl -- crd) \
     charts/mikebom-operator/crds/namespacescan.kusari.dev_v1.yaml
```

## Failure-mode contract

- **Operator library compile error**: `cargo run --bin mikebom-operator-ctl` fails to build; the failure surfaces before the `crd` subcommand is invoked. Not the subcommand's responsibility.
- **`--output PATH` parent dir missing**: exits 1, writes a message to stderr naming the missing directory. Does not create directories implicitly.
- **`--output PATH` exists**: overwrites silently. Callers wanting "no clobber" should check existence themselves.
- **No `kubectl`/`kopium`/cluster connection required**: the subcommand is pure Rust + in-process serialization. Runs in any environment that can build the workspace.
