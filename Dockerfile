# syntax=docker/dockerfile:1.7
# Multi-stage build for billing-server-rs.
#
# The crate's three OreSoftware path dependencies live in the private
# k8s-libs-and-shared-defs repository. CI checks that repository out at a pinned
# commit under .build/shared-libs before invoking BuildKit. The directory layout
# below intentionally recreates the monorepo-relative path expected by Cargo:
#
#   /workspace/remote/services/billing-server-rs
#   /workspace/remote/libs/{telemetry-rs,wal-consumer-rs,nats/...}
#
# Nothing is compiled in Kubernetes. The runtime stage contains only the
# release binary, CA certificates, and the declarative schema reference.

FROM rust:1.95-bookworm AS build
ARG TARGETARCH

WORKDIR /workspace/remote
COPY .build/shared-libs ./libs

WORKDIR /workspace/remote/services/billing-server-rs
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY schema ./schema
COPY generated ./generated

# Fail with a targeted message if CI forgot the private shared-libs checkout,
# rather than producing a misleading Cargo path-dependency error later.
RUN test -f ../../libs/telemetry-rs/Cargo.toml \
 && test -f ../../libs/wal-consumer-rs/Cargo.toml \
 && test -f ../../libs/nats/subject-defs/generated/rust/Cargo.toml

RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    --mount=type=cache,target=/workspace/remote/services/billing-server-rs/target,id=billing-server-rs-target-${TARGETARCH},sharing=locked \
    cargo build --release --locked --bin billing-server-rs \
 && install -m 0555 target/release/billing-server-rs /usr/local/bin/billing-server-rs

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && apt-get clean \
  && rm -rf /var/lib/apt/lists/*

COPY --from=build /usr/local/bin/billing-server-rs /usr/local/bin/billing-server-rs
COPY --from=build /workspace/remote/services/billing-server-rs/schema /opt/billing-server-rs/schema

ENV BILLING_HOST=0.0.0.0
ENV BILLING_PORT=8087

EXPOSE 8087

# 65532 is the conventional nonroot uid/gid used by the restricted workload
# manifests. The binary and schema are world-readable; no writable application
# directory is needed at runtime.
USER 65532:65532

ENTRYPOINT ["/usr/local/bin/billing-server-rs"]
