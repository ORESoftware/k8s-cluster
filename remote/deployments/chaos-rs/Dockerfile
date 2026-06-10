# Build from the k8s-cluster repository root so the path dependency resolves:
#   docker build -f remote/deployments/chaos-rs/Dockerfile -t dd-chaos:dev .
FROM rust:1.90-bookworm AS build
WORKDIR /repo
COPY remote/deployments/chaos-rs ./remote/deployments/chaos-rs
COPY remote/libs/nats/subject-defs/generated/rust ./remote/libs/nats/subject-defs/generated/rust
WORKDIR /repo/remote/deployments/chaos-rs
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/*
COPY --from=build /repo/remote/deployments/chaos-rs/target/release/dd-chaos /usr/local/bin/dd-chaos
ENV HOST=0.0.0.0 PORT=8133
EXPOSE 8133
ENTRYPOINT ["/usr/local/bin/dd-chaos"]
