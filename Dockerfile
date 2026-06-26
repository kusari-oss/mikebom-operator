FROM --platform=$BUILDPLATFORM rust:1-bookworm AS builder
WORKDIR /src
COPY . .
RUN cargo build --release --bin mikebom-operator

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /src/target/release/mikebom-operator /usr/local/bin/mikebom-operator
USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/mikebom-operator"]
