# Base images pinned to manifest-list digests. Tags retained for human
# reference; Docker validates the digest matches before pulling.
# Refresh via `crane digest <ref>` and re-pin in a PR — never let a base
# image drift silently.
FROM --platform=$BUILDPLATFORM rust:1-bookworm@sha256:5e2214abe154fe26e39f64488952e5c991eeed1d6d6da7cc8381ae83927f0cfc AS builder
WORKDIR /src
COPY . .
RUN cargo build --release --bin mikebom-operator

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:b0ae8e989418b458e0f25489bc3be523718938a2b70864cc0f6a00af1ddbd985
COPY --from=builder /src/target/release/mikebom-operator /usr/local/bin/mikebom-operator
USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/mikebom-operator"]
