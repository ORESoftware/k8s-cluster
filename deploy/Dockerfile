# syntax=docker/dockerfile:1
#
# dd-fabrication-server — production image.
#
# BUILD CONTEXT (important): this crate has FIVE path dependencies that resolve
# OUTSIDE this repository:
#
#   dd-telemetry            = { path = "../../libs/telemetry-rs" }
#   des_engine              = { path = "../../submodules/discrete-event-system.rs" }
#   dd-nats-subject-defs    = { path = "../../libs/nats/subject-defs/generated/rust" }
#   dd-runtime-config-client= { path = "../../libs/runtime-config-client-rs" }
#     └─ which itself pulls dd-shared-interfaces
#        = { path = "../interfaces/shared/generated/rust" }
#
# Per AGENTS.md those resolve only when this repo is checked out at
# `remote/deployments/fabrication-server-rs` inside the ORESoftware/k8s-cluster
# superproject. The build context is therefore the *superproject root*, not this
# repo root — the same convention used by the existing multi-path-dep services
# (see k8s-cluster `remote/deployments/cluster-mcp-rs/Dockerfile` and
# `remote/deployments/usacc-rest-api-backend-rs/Dockerfile`).
#
#   cd ~/codes/ores/k8s-cluster
#   docker build \
#     -f remote/deployments/fabrication-server-rs/deploy/Dockerfile \
#     -t ghcr.io/oresoftware/dd-fabrication-server:dev .
#
# A build from this repo alone CANNOT succeed; there is no vendoring step. This
# replaces the legacy hostPath + in-pod `cargo build` deployment
# (k8s-cluster remote/argocd/dd-next-runtime/dd-fabrication-server.deployment.yaml,
# currently scaled to 0 replicas).
#
# Base images are pinned by digest for reproducible, tamper-evident builds.
# Refresh with: docker buildx imagetools inspect <tag>
FROM rust:1.95-bookworm@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1 AS build
ARG TARGETARCH
WORKDIR /app

# Out-of-repo path deps first: they change least often, so they cache best.
COPY remote/libs/telemetry-rs ./remote/libs/telemetry-rs
COPY remote/libs/nats/subject-defs/generated/rust ./remote/libs/nats/subject-defs/generated/rust
COPY remote/libs/runtime-config-client-rs ./remote/libs/runtime-config-client-rs
COPY remote/libs/interfaces/shared/generated/rust ./remote/libs/interfaces/shared/generated/rust
COPY remote/submodules/discrete-event-system.rs ./remote/submodules/discrete-event-system.rs

COPY remote/deployments/fabrication-server-rs/Cargo.toml \
     remote/deployments/fabrication-server-rs/Cargo.lock \
     ./remote/deployments/fabrication-server-rs/
COPY remote/deployments/fabrication-server-rs/src ./remote/deployments/fabrication-server-rs/src
# src/lib.rs include_str!s generated/api-docs.{html,json}; the build fails
# without them.
COPY remote/deployments/fabrication-server-rs/generated ./remote/deployments/fabrication-server-rs/generated

WORKDIR /app/remote/deployments/fabrication-server-rs
# BuildKit cache mounts per remote/deployments/DOCKERFILE-CACHING.md. The
# cache-mounted target/ is NOT part of the image, so binaries must be copied to
# a real path inside the same RUN. Two binaries: the crate default
# (dd-fabrication-server) and src/bin/dd-fabrication-web-server.rs.
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    --mount=type=cache,target=/app/remote/deployments/fabrication-server-rs/target,id=fabrication-server-rs-target-${TARGETARCH},sharing=locked \
    cargo build --release --locked \
 && cp target/release/dd-fabrication-server /usr/local/bin/dd-fabrication-server \
 && cp target/release/dd-fabrication-web-server /usr/local/bin/dd-fabrication-web-server

FROM debian:bookworm-slim@sha256:96e378d7e6531ac9a15ad505478fcc2e69f371b10f5cdf87857c4b8188404716
# rustls (sea-orm, reqwest, tokio-tungstenite, async-nats TLS) needs the system
# trust store.
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && apt-get clean && rm -rf /var/lib/apt/lists/*

COPY --from=build /usr/local/bin/dd-fabrication-server /usr/local/bin/dd-fabrication-server
COPY --from=build /usr/local/bin/dd-fabrication-web-server /usr/local/bin/dd-fabrication-web-server

ENV HOST=0.0.0.0
# src/config.rs: PORT is the HTTP/Axum listener, FABRICATION_TCP_PORT the raw
# TCP transport listener.
ENV PORT=8113
ENV FABRICATION_TCP_PORT=8114

EXPOSE 8113
EXPOSE 8114

# Matches the pod securityContext (runAsUser/runAsGroup 10001) so the image is
# valid under a `restricted` PodSecurity namespace with runAsNonRoot.
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/dd-fabrication-server"]
