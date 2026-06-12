# syntax=docker/dockerfile:1
FROM rust:1.90-bookworm AS build
ARG TARGETARCH

WORKDIR /app

# Build context must be the repo root:
#   docker build -f remote/deployments/dataset-labeling-rs/Dockerfile -t dd-dataset-labeling:dev .
COPY remote/libs/nats/subject-defs/generated/rust /app/remote/libs/nats/subject-defs/generated/rust
COPY remote/deployments/dataset-labeling-rs/Cargo.toml /app/remote/deployments/dataset-labeling-rs/Cargo.toml
COPY remote/deployments/dataset-labeling-rs/Cargo.lock /app/remote/deployments/dataset-labeling-rs/Cargo.lock
COPY remote/deployments/dataset-labeling-rs/src /app/remote/deployments/dataset-labeling-rs/src
COPY remote/deployments/dataset-labeling-rs/generated /app/remote/deployments/dataset-labeling-rs/generated

WORKDIR /app/remote/deployments/dataset-labeling-rs
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    --mount=type=cache,target=/app/remote/deployments/dataset-labeling-rs/target,id=dataset-labeling-rs-target-${TARGETARCH},sharing=locked \
    cargo build --release --locked \
 && cp target/release/dd-dataset-labeling /usr/local/bin/dd-dataset-labeling

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && apt-get clean
COPY --from=build /usr/local/bin/dd-dataset-labeling /usr/local/bin/dd-dataset-labeling

ENV HOST=0.0.0.0
ENV PORT=8138

EXPOSE 8138

CMD ["/usr/local/bin/dd-dataset-labeling"]
