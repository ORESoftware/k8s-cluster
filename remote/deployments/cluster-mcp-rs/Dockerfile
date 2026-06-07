FROM rust:1.90-bookworm AS build

WORKDIR /app

# Build context must be the repo root:
#   docker build -f remote/deployments/cluster-mcp-rs/Dockerfile -t dd-cluster-mcp-rs:dev .
COPY remote/libs/runtime-config-client-rs /app/remote/libs/runtime-config-client-rs
COPY remote/deployments/cluster-mcp-rs/Cargo.toml /app/remote/deployments/cluster-mcp-rs/Cargo.toml
COPY remote/deployments/cluster-mcp-rs/Cargo.lock /app/remote/deployments/cluster-mcp-rs/Cargo.lock
COPY remote/deployments/cluster-mcp-rs/generated /app/remote/deployments/cluster-mcp-rs/generated
COPY remote/deployments/cluster-mcp-rs/src /app/remote/deployments/cluster-mcp-rs/src

WORKDIR /app/remote/deployments/cluster-mcp-rs
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && apt-get clean

COPY --from=build /app/remote/deployments/cluster-mcp-rs/target/release/dd-cluster-mcp-rs /usr/local/bin/dd-cluster-mcp-rs

ENV HOST=0.0.0.0
ENV PORT=8091

EXPOSE 8091

USER 10001:10001
CMD ["/usr/local/bin/dd-cluster-mcp-rs"]
