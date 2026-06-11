# syntax=docker/dockerfile:1
FROM rust:1.90-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    --mount=type=cache,target=/app/target,id=bastion-rs-target,sharing=locked \
    cargo build --release \
 && cp target/release/dd-bastion /usr/local/bin/dd-bastion

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates curl \
  && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/dd-bastion /usr/local/bin/dd-bastion
ENTRYPOINT ["/usr/local/bin/dd-bastion"]
