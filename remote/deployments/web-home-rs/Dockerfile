# syntax=docker/dockerfile:1.7

FROM rust:1.90-bookworm AS build
WORKDIR /app/deployments/web-home-rs

COPY deployments/web-home-rs/Cargo.toml deployments/web-home-rs/Cargo.lock ./
COPY libs/runtime-config-client-rs /app/libs/runtime-config-client-rs
COPY libs/interfaces/shared/generated/rust /app/libs/interfaces/shared/generated/rust
COPY libs/nats/subject-defs/generated/rust /app/libs/nats/subject-defs/generated/rust
COPY deployments/generated-api-docs-index.html deployments/generated-api-docs-index.json /app/deployments/
COPY deployments/web-home-rs/generated ./generated
COPY deployments/web-home-rs/src ./src
RUN cargo build --release

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && apt-get clean

COPY --from=build /app/deployments/web-home-rs/target/release/dd-remote-web-home /usr/local/bin/dd-remote-web-home

ENV HOST=0.0.0.0 \
    PORT=8080

EXPOSE 8080
USER 10001:10001
CMD ["/usr/local/bin/dd-remote-web-home"]
