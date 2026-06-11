# syntax=docker/dockerfile:1.7

# Self-contained build. Build context is this repo root:
#   docker build -t sonus-auris-backend:dev .
FROM rust:1.91.1-bookworm AS build
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY generated ./generated
COPY src ./src

RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    --mount=type=cache,target=/app/target,id=sonus-auris-backend-target,sharing=locked \
    cargo build --release --locked \
 && cp target/release/dd-sound-recorder-rs /usr/local/bin/dd-sound-recorder-rs

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && apt-get clean && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/dd-sound-recorder-rs /usr/local/bin/dd-sound-recorder-rs

ENV HOST=0.0.0.0
ENV PORT=8126

EXPOSE 8126
USER 10001:10001
CMD ["/usr/local/bin/dd-sound-recorder-rs"]
