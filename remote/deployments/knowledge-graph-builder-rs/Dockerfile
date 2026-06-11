FROM rust:1.90-bookworm AS build

WORKDIR /app

# Build context must be the repo root:
#   docker build -f remote/deployments/knowledge-graph-builder-rs/Dockerfile -t dd-knowledge-graph-builder:dev .
COPY remote/libs/nats/subject-defs/generated/rust /app/remote/libs/nats/subject-defs/generated/rust
COPY remote/deployments/knowledge-graph-builder-rs/Cargo.toml /app/remote/deployments/knowledge-graph-builder-rs/Cargo.toml
COPY remote/deployments/knowledge-graph-builder-rs/Cargo.lock /app/remote/deployments/knowledge-graph-builder-rs/Cargo.lock
COPY remote/deployments/knowledge-graph-builder-rs/src /app/remote/deployments/knowledge-graph-builder-rs/src
COPY remote/deployments/knowledge-graph-builder-rs/generated /app/remote/deployments/knowledge-graph-builder-rs/generated

WORKDIR /app/remote/deployments/knowledge-graph-builder-rs
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && apt-get clean
COPY --from=build /app/remote/deployments/knowledge-graph-builder-rs/target/release/dd-knowledge-graph-builder /usr/local/bin/dd-knowledge-graph-builder

ENV HOST=0.0.0.0
ENV PORT=8137

EXPOSE 8137

CMD ["/usr/local/bin/dd-knowledge-graph-builder"]
