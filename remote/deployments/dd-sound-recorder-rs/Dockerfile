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
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && apt-get clean
COPY --from=build /app/remote/deployments/dd-sound-recorder-rs/target/release/dd-sound-recorder-rs /usr/local/bin/dd-sound-recorder-rs

ENV HOST=0.0.0.0
ENV PORT=8126

EXPOSE 8126
USER 10001:10001
CMD ["/usr/local/bin/dd-sound-recorder-rs"]
