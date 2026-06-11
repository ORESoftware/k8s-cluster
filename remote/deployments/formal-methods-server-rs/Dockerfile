# syntax=docker/dockerfile:1
FROM rust:1.90-bookworm AS build
WORKDIR /repo
COPY remote/deployments/formal-methods-server-rs ./remote/deployments/formal-methods-server-rs
COPY remote/libs/runtime-config-client-rs ./remote/libs/runtime-config-client-rs
WORKDIR /repo/remote/deployments/formal-methods-server-rs
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    --mount=type=cache,target=/repo/remote/deployments/formal-methods-server-rs/target,id=formal-methods-server-rs-target,sharing=locked \
    cargo build --release \
 && cp target/release/dd-formal-methods-server /usr/local/bin/dd-formal-methods-server

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates git openssh-client z3 \
  && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/dd-formal-methods-server /usr/local/bin/dd-formal-methods-server
ENTRYPOINT ["/usr/local/bin/dd-formal-methods-server"]
