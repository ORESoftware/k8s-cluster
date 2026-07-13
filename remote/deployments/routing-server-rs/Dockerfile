# syntax=docker/dockerfile:1
# Build from the k8s-cluster repository root so the path dependency resolves:
#   docker build -f remote/deployments/routing-server-rs/Dockerfile -t dd-routing-server:dev .
FROM rust:1.90-bookworm AS build
ARG TARGETARCH
WORKDIR /repo
COPY remote/deployments/routing-server-rs ./remote/deployments/routing-server-rs
COPY remote/libs/nats/subject-defs/generated/rust ./remote/libs/nats/subject-defs/generated/rust
WORKDIR /repo/remote/deployments/routing-server-rs
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    --mount=type=cache,target=/repo/remote/deployments/routing-server-rs/target,id=routing-server-rs-target-${TARGETARCH},sharing=locked \
    cargo build --release \
 && cp target/release/dd-routing-server /usr/local/bin/dd-routing-server

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/dd-routing-server /usr/local/bin/dd-routing-server
ENV HOST=0.0.0.0 PORT=8132
EXPOSE 8132
ENTRYPOINT ["/usr/local/bin/dd-routing-server"]
