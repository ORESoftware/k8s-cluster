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
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && apt-get clean
COPY --from=build /app/remote/deployments/runtime-config-rs/target/release/dd-runtime-config /usr/local/bin/dd-runtime-config

ENV HOST=0.0.0.0
ENV PORT=8110

EXPOSE 8110

CMD ["/usr/local/bin/dd-runtime-config"]
