# syntax=docker/dockerfile:1
# Multi-stage build for akrion-web-server.
FROM rust:1-slim-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev
WORKDIR /build/akrion-web-server.rs
COPY . .
RUN cargo build --release --bin akrion-web-server && strip target/release/akrion-web-server

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build --chown=65532:65532 /build/akrion-web-server.rs/target/release/akrion-web-server /usr/local/bin/akrion-web-server
COPY --from=build --chown=65532:65532 /build/akrion-web-server.rs/assets /app/assets
WORKDIR /app
EXPOSE 8124
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/akrion-web-server"]
