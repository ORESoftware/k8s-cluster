# Build from the k8s-cluster repository root so path dependencies resolve:
#   docker build -f remote/deployments/mip-solver-node.rs/Dockerfile .
FROM rust:1.90-bookworm AS build
WORKDIR /repo
COPY remote/deployments/mip-solver-node.rs ./remote/deployments/mip-solver-node.rs
COPY remote/libs/nats/subject-defs/generated/rust ./remote/libs/nats/subject-defs/generated/rust
COPY remote/submodules/discrete-event-system.rs ./remote/submodules/discrete-event-system.rs
WORKDIR /repo/remote/deployments/mip-solver-node.rs
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/*
COPY --from=build /repo/remote/deployments/mip-solver-node.rs/target/release/dd-in-house-mip-solver-node /usr/local/bin/dd-in-house-mip-solver-node
ENV HOST=0.0.0.0 PORT=8117
EXPOSE 8117
ENTRYPOINT ["/usr/local/bin/dd-in-house-mip-solver-node"]
