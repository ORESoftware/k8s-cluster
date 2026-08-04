# syntax=docker/dockerfile:1.7
#
# Self-contained build of the zero-knowledge sync server. Build context is the
# repo root:
#   docker build -f deploy/Dockerfile -t threefa-sync-server:dev .
# Base images pinned by digest for reproducible, tamper-evident builds. Refresh
# the digest when bumping the tag: `docker buildx imagetools inspect <tag>`.
FROM rust:1.95-bookworm@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1 AS build
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/app/target,id=threefa-backend-target,sharing=locked \
    cargo build --release --locked \
 && cp target/release/threefa-sync-server /usr/local/bin/threefa-sync-server

FROM debian:bookworm-slim@sha256:96e378d7e6531ac9a15ad505478fcc2e69f371b10f5cdf87857c4b8188404716
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && apt-get clean && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /usr/local/bin/threefa-sync-server /usr/local/bin/threefa-sync-server
# The service resolves executable-owned policy from ../share/threefa-backend;
# WORKDIR is deliberately not part of the production trust boundary.
COPY .cli-flags.toml /usr/local/share/threefa-backend/.cli-flags.toml

ENV BIND_ADDR=0.0.0.0:8080
EXPOSE 8080
USER 10001:10001
CMD ["/usr/local/bin/threefa-sync-server"]
