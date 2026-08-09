ARG RUST_IMAGE=docker.io/library/rust:1.90.0-bookworm@sha256:3914072ca0c3b8aad871db9169a651ccfce30cf58303e5d6f2db16d1d8a7e58f
ARG RUNTIME_IMAGE=docker.io/library/debian:bookworm-slim@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818

FROM ${RUST_IMAGE} AS builder
WORKDIR /workspace

ENV CARGO_INCREMENTAL=0 \
    CARGO_TERM_COLOR=always \
    RUST_BACKTRACE=1

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN set -eux; \
    cargo build --locked --release --bin gha-capacity-broker; \
    install -D -m 0555 target/release/gha-capacity-broker /out/gha-capacity-broker; \
    test ! -e /out/cargo; \
    test ! -e /out/rustc

FROM ${RUNTIME_IMAGE} AS runtime
ARG OCI_CREATED
ARG OCI_REVISION
ARG OCI_SOURCE=https://github.com/ORESoftware/k8s-cluster

LABEL org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.description="GitHub Actions billing-aware capacity broker for official ARC and bounded CI continuity" \
      org.opencontainers.image.licenses="UNLICENSED" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.source="${OCI_SOURCE}" \
      org.opencontainers.image.title="ORESoftware GHA capacity broker" \
      org.opencontainers.image.vendor="ORESoftware"

COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=builder --chown=65532:65532 /out/gha-capacity-broker /usr/local/bin/gha-capacity-broker

ENV RUST_LOG=gha_capacity_broker=info \
    SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt

WORKDIR /app
USER 65532:65532
STOPSIGNAL SIGTERM
EXPOSE 8117
ENTRYPOINT ["/usr/local/bin/gha-capacity-broker"]
