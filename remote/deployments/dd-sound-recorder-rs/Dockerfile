# syntax=docker/dockerfile:1.7

FROM rust:1.91.1-bookworm AS build
WORKDIR /app

# Build context should be the repo root:
#   docker build -f remote/deployments/dd-sound-recorder-rs/Dockerfile -t dd-sound-recorder-rs:dev .
COPY remote/libs/pg-defs/generated/rust /app/remote/libs/pg-defs/generated/rust
COPY remote/deployments/dd-sound-recorder-rs/Cargo.toml /app/remote/deployments/dd-sound-recorder-rs/Cargo.toml
COPY remote/deployments/dd-sound-recorder-rs/Cargo.lock /app/remote/deployments/dd-sound-recorder-rs/Cargo.lock
COPY remote/deployments/dd-sound-recorder-rs/generated /app/remote/deployments/dd-sound-recorder-rs/generated
COPY remote/deployments/dd-sound-recorder-rs/src /app/remote/deployments/dd-sound-recorder-rs/src

WORKDIR /app/remote/deployments/dd-sound-recorder-rs
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    --mount=type=cache,target=/app/remote/deployments/dd-sound-recorder-rs/target,id=dd-sound-recorder-rs-target,sharing=locked \
    cargo build --release --locked \
 && cp target/release/dd-sound-recorder-rs /usr/local/bin/dd-sound-recorder-rs

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && apt-get clean
COPY --from=build /usr/local/bin/dd-sound-recorder-rs /usr/local/bin/dd-sound-recorder-rs

ENV HOST=0.0.0.0
ENV PORT=8126

EXPOSE 8126
USER 10001:10001
CMD ["/usr/local/bin/dd-sound-recorder-rs"]
