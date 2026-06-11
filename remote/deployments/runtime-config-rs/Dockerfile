# syntax=docker/dockerfile:1
FROM rust:1.86-bookworm AS build

WORKDIR /app

# Bring in the shared interfaces crate as a sibling path dep (Cargo.toml
# resolves it as `../../libs/interfaces/shared/generated/rust`). The build
# context for `docker build` must therefore be the repo root, e.g.:
#   docker build -f remote/deployments/runtime-config-rs/Dockerfile -t dd-runtime-config:dev .
COPY remote/libs/interfaces/shared/generated/rust /app/remote/libs/interfaces/shared/generated/rust
COPY remote/deployments/runtime-config-rs/Cargo.toml /app/remote/deployments/runtime-config-rs/Cargo.toml
COPY remote/deployments/runtime-config-rs/src /app/remote/deployments/runtime-config-rs/src

WORKDIR /app/remote/deployments/runtime-config-rs
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    --mount=type=cache,target=/app/remote/deployments/runtime-config-rs/target,id=runtime-config-rs-target,sharing=locked \
    cargo build --release \
 && cp target/release/dd-runtime-config /usr/local/bin/dd-runtime-config

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && apt-get clean
COPY --from=build /usr/local/bin/dd-runtime-config /usr/local/bin/dd-runtime-config

ENV HOST=0.0.0.0
ENV PORT=8110

EXPOSE 8110

CMD ["/usr/local/bin/dd-runtime-config"]
