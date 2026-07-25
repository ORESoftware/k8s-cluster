# syntax=docker/dockerfile:1
FROM rust:1.90-bookworm AS build
ARG TARGETARCH

WORKDIR /app

# Build context must be the repo root:
#   docker build -f remote/deployments/browser-mcp-rs/Dockerfile -t dd-browser-mcp-rs:dev .
COPY remote/libs/telemetry-rs /app/remote/libs/telemetry-rs
COPY remote/libs/runtime-config-client-rs /app/remote/libs/runtime-config-client-rs
COPY remote/deployments/browser-mcp-rs/Cargo.toml /app/remote/deployments/browser-mcp-rs/Cargo.toml
COPY remote/deployments/browser-mcp-rs/Cargo.lock /app/remote/deployments/browser-mcp-rs/Cargo.lock
COPY remote/deployments/browser-mcp-rs/src /app/remote/deployments/browser-mcp-rs/src

WORKDIR /app/remote/deployments/browser-mcp-rs
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    --mount=type=cache,target=/app/remote/deployments/browser-mcp-rs/target,id=browser-mcp-rs-target-${TARGETARCH},sharing=locked \
    cargo build --release --locked \
 && cp target/release/dd-browser-mcp-rs /usr/local/bin/dd-browser-mcp-rs

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && apt-get clean

COPY --from=build /usr/local/bin/dd-browser-mcp-rs /usr/local/bin/dd-browser-mcp-rs

ENV HOST=0.0.0.0
ENV PORT=8092

EXPOSE 8092

USER 10001:10001
CMD ["/usr/local/bin/dd-browser-mcp-rs"]
