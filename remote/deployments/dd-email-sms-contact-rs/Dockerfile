# syntax=docker/dockerfile:1
# Multi-stage build (mirrors rest-api-rs). The cluster also supports building this in-cluster from
# the mounted repo via `cargo run --release`; this Dockerfile is for a prebuilt-image path.
FROM rust:1.90-bookworm AS build
WORKDIR /app
COPY Cargo.toml ./Cargo.toml
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    --mount=type=cache,target=/app/target,id=dd-email-sms-contact-rs-target,sharing=locked \
    cargo build --release \
 && cp target/release/dd-email-sms-contact-rs /usr/local/bin/dd-email-sms-contact-rs

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && apt-get clean && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/dd-email-sms-contact-rs /usr/local/bin/dd-email-sms-contact-rs
ENV HOST=0.0.0.0
ENV PORT=8120
EXPOSE 8120
CMD ["/usr/local/bin/dd-email-sms-contact-rs"]
