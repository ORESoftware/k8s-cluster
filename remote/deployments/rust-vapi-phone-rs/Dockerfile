# syntax=docker/dockerfile:1
FROM rust:1.90-bookworm AS build

WORKDIR /app

# The shared runtime-config client is a sibling path dep
# (`../../libs/runtime-config-client-rs`), so the build context for
# `docker build` must be the repo root, e.g.:
#   docker build -f remote/deployments/rust-vapi-phone-rs/Dockerfile -t dd-rust-vapi-phone:dev .
COPY remote/libs/runtime-config-client-rs /app/remote/libs/runtime-config-client-rs
COPY remote/libs/interfaces/redis/generated/rust /app/remote/libs/interfaces/redis/generated/rust
COPY remote/deployments/rest-api-rs/certs /app/remote/deployments/rest-api-rs/certs
COPY remote/deployments/rust-vapi-phone-rs/Cargo.toml /app/remote/deployments/rust-vapi-phone-rs/Cargo.toml
COPY remote/deployments/rust-vapi-phone-rs/Cargo.lock /app/remote/deployments/rust-vapi-phone-rs/Cargo.lock
COPY remote/deployments/rust-vapi-phone-rs/src /app/remote/deployments/rust-vapi-phone-rs/src
COPY remote/deployments/rust-vapi-phone-rs/generated /app/remote/deployments/rust-vapi-phone-rs/generated

WORKDIR /app/remote/deployments/rust-vapi-phone-rs
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    --mount=type=cache,target=/app/remote/deployments/rust-vapi-phone-rs/target,id=rust-vapi-phone-rs-target,sharing=locked \
    cargo build --release --locked \
 && cp target/release/dd-rust-vapi-phone /usr/local/bin/dd-rust-vapi-phone

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && apt-get clean
COPY --from=build /usr/local/bin/dd-rust-vapi-phone /usr/local/bin/dd-rust-vapi-phone

ENV HOST=0.0.0.0
ENV PORT=8113

EXPOSE 8113

CMD ["/usr/local/bin/dd-rust-vapi-phone"]
