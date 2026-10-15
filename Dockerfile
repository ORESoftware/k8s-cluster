# Build from the k8s-cluster repository root so path dependencies resolve:
#   docker build -f remote/deployments/mip-solver-node.rs/Dockerfile .
FROM rust:1.90-bookworm AS build
WORKDIR /repo
COPY remote/deployments/mip-solver-node.rs ./remote/deployments/mip-solver-node.rs
COPY remote/submodules/discrete-event-system.rs ./remote/submodules/discrete-event-system.rs
WORKDIR /repo/remote/deployments/mip-solver-node.rs
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/* \
  && install -d /usr/local/share/dd-in-house-mip-solver-node
WORKDIR /app
COPY --from=build /repo/remote/deployments/mip-solver-node.rs/target/release/dd-in-house-mip-solver-node /usr/local/bin/dd-in-house-mip-solver-node
COPY --from=build /repo/remote/deployments/mip-solver-node.rs/.cli-flags.toml /usr/local/share/dd-in-house-mip-solver-node/.cli-flags.toml
ENV HOST=0.0.0.0 \
    PORT=8117 \
    FLAGS2ENV_CONFIG=/usr/local/share/dd-in-house-mip-solver-node/.cli-flags.toml
EXPOSE 8117
ENTRYPOINT ["/usr/local/bin/dd-in-house-mip-solver-node"]
