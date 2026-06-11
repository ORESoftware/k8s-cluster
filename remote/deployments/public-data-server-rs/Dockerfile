# syntax=docker/dockerfile:1
FROM rust:1.90-bookworm AS build

WORKDIR /app

# Build context must be the repo root:
#   docker build -f remote/deployments/public-data-server-rs/Dockerfile -t dd-public-data-server:dev .
COPY remote/libs/nats/subject-defs/generated/rust /app/remote/libs/nats/subject-defs/generated/rust
COPY remote/deployments/public-data-server-rs/Cargo.toml /app/remote/deployments/public-data-server-rs/Cargo.toml
COPY remote/deployments/public-data-server-rs/Cargo.lock /app/remote/deployments/public-data-server-rs/Cargo.lock
COPY remote/deployments/public-data-server-rs/src /app/remote/deployments/public-data-server-rs/src
COPY remote/deployments/public-data-server-rs/generated /app/remote/deployments/public-data-server-rs/generated

WORKDIR /app/remote/deployments/public-data-server-rs
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    --mount=type=cache,target=/app/remote/deployments/public-data-server-rs/target,id=public-data-server-rs-target,sharing=locked \
    cargo build --release --locked \
 && cp target/release/dd-public-data-server /usr/local/bin/dd-public-data-server

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && apt-get clean
COPY --from=build /usr/local/bin/dd-public-data-server /usr/local/bin/dd-public-data-server

ENV HOST=0.0.0.0
ENV PORT=8115

EXPOSE 8115

CMD ["/usr/local/bin/dd-public-data-server"]
