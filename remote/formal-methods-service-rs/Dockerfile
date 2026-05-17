# Multi-stage build for remote/formal-methods-service-rs.
#
# The cluster's Argo CD deployment does not use this image today: the
# Deployment manifest mounts the repo via hostPath and runs
# `cargo run --release` from the official `rust:bookworm` image (mirroring
# the dd-contract-service / dd-formal-methods-server pattern). This
# Dockerfile is kept for local image-based runs and for environments that
# prefer a built artifact (`docker build -t dd-formal-methods-service .`).

ARG RUST_VERSION=1.88.0
ARG DEBIAN_RELEASE=bookworm

FROM rust:${RUST_VERSION}-${DEBIAN_RELEASE} AS builder
WORKDIR /build

COPY Cargo.toml ./Cargo.toml
COPY rust-toolchain.toml ./rust-toolchain.toml

RUN mkdir -p src tests && \
    echo "fn main() {}" > src/main.rs && \
    echo "// dep cache" > src/lib.rs && \
    cargo build --release || true

COPY src ./src
COPY tests ./tests
COPY rustfmt.toml ./rustfmt.toml

RUN cargo build --release && \
    strip target/release/dd-formal-methods-service

FROM debian:${DEBIAN_RELEASE}-slim AS runtime

ARG RELEASE_VERSION=development
ENV RELEASE_VERSION=${RELEASE_VERSION}

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates \
        git \
        dumb-init \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd --system --gid 1001 fmgroup && \
    useradd --system --uid 1001 --gid fmgroup --home /app --shell /usr/sbin/nologin fmuser

WORKDIR /app

COPY --from=builder /build/target/release/dd-formal-methods-service /usr/local/bin/dd-formal-methods-service

RUN mkdir -p /app/.work && chown -R fmuser:fmgroup /app

USER fmuser

ENV PORT=8111
EXPOSE 8111

HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD wget -qO- "http://localhost:${PORT}/health" >/dev/null 2>&1 || exit 1

ENTRYPOINT ["dumb-init", "--"]
CMD ["/usr/local/bin/dd-formal-methods-service"]
