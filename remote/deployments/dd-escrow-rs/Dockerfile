# syntax=docker/dockerfile:1
FROM rust:1.90-bookworm AS build
ARG TARGETARCH
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY generated ./generated
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    --mount=type=cache,target=/app/target,id=dd-escrow-rs-target-${TARGETARCH},sharing=locked \
    cargo build --release \
 && cp target/release/dd-escrow-rs /usr/local/bin/dd-escrow-rs

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates
COPY --from=build /usr/local/bin/dd-escrow-rs /usr/local/bin/dd-escrow-rs
ENTRYPOINT ["/usr/local/bin/dd-escrow-rs"]
