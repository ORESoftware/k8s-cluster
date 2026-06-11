# syntax=docker/dockerfile:1
# Multi-stage build for billing-server-rs.
#
# Stage 1: build the release binary against a pinned Rust toolchain.
# Stage 2: minimal runtime image with only the binary, migrations,
#          and CA certs (we make outbound TLS calls to Solana RPC,
#          Stripe, PayPal, Plaid, etc.).

FROM rust:1.95-bookworm AS build

WORKDIR /app

# Dependency downloads + compiled artifacts are cached across builds via the
# BuildKit cargo cache mounts on the build RUN below.
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    --mount=type=cache,target=/app/target,id=billing-server-rs-target,sharing=locked \
    cargo build --release --bin billing-server-rs \
 && cp target/release/billing-server-rs /usr/local/bin/billing-server-rs

FROM debian:bookworm-slim

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && apt-get clean \
  && rm -rf /var/lib/apt/lists/*

COPY --from=build /usr/local/bin/billing-server-rs /usr/local/bin/billing-server-rs
COPY --from=build /app/migrations /opt/billing-server-rs/migrations

ENV BILLING_HOST=0.0.0.0
ENV BILLING_PORT=8087

EXPOSE 8087

USER 65532:65532

CMD ["/usr/local/bin/billing-server-rs"]
