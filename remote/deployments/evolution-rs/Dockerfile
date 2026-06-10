# Build from the k8s-cluster repository root so the path dependency resolves:
#   docker build -f remote/deployments/evolution-rs/Dockerfile -t dd-evolution-optimizer:dev .
FROM rust:1.90-bookworm AS build
WORKDIR /repo
COPY remote/deployments/evolution-rs ./remote/deployments/evolution-rs
COPY remote/libs/nats/subject-defs/generated/rust ./remote/libs/nats/subject-defs/generated/rust
WORKDIR /repo/remote/deployments/evolution-rs
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/*
COPY --from=build /repo/remote/deployments/evolution-rs/target/release/dd-evolution-optimizer /usr/local/bin/dd-evolution-optimizer
ENV HOST=0.0.0.0 PORT=8131
EXPOSE 8131
ENTRYPOINT ["/usr/local/bin/dd-evolution-optimizer"]
