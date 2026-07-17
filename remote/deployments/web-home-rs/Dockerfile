# syntax=docker/dockerfile:1.7

FROM rust:1.90-bookworm AS build
ARG TARGETARCH
WORKDIR /app/deployments/web-home-rs

COPY deployments/web-home-rs/Cargo.toml deployments/web-home-rs/Cargo.lock ./
COPY libs/runtime-config-client-rs /app/libs/runtime-config-client-rs
COPY libs/interfaces/shared/generated/rust /app/libs/interfaces/shared/generated/rust
COPY libs/nats/subject-defs/generated/rust /app/libs/nats/subject-defs/generated/rust
COPY libs/telemetry-rs /app/libs/telemetry-rs
COPY libs/browser/service-worker.js /app/libs/browser/service-worker.js
COPY deployments/generated-api-docs-index.html deployments/generated-api-docs-index.json /app/deployments/
COPY deployments/web-home-rs/generated ./generated
COPY deployments/web-home-rs/src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    --mount=type=cache,target=/app/deployments/web-home-rs/target,id=web-home-rs-target-${TARGETARCH},sharing=locked \
    cargo build --release \
 && cp target/release/dd-remote-web-home /usr/local/bin/dd-remote-web-home

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && apt-get clean

COPY --from=build /usr/local/bin/dd-remote-web-home /usr/local/bin/dd-remote-web-home

ENV HOST=0.0.0.0 \
    PORT=8080

EXPOSE 8080
USER 10001:10001
CMD ["/usr/local/bin/dd-remote-web-home"]
