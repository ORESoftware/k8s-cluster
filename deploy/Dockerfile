# syntax=docker/dockerfile:1.7
#
# Self-contained build of the zero-knowledge sync server. Build context is the
# repo root:
#   docker build -f deploy/Dockerfile -t threefa-sync-server:dev .
FROM rust:1.83-bookworm AS build
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations

RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/app/target,id=threefa-backend-target,sharing=locked \
    cargo build --release --locked \
 && cp target/release/threefa-sync-server /usr/local/bin/threefa-sync-server

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && apt-get clean && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/threefa-sync-server /usr/local/bin/threefa-sync-server

ENV BIND_ADDR=0.0.0.0:8080
EXPOSE 8080
USER 10001:10001
CMD ["/usr/local/bin/threefa-sync-server"]
