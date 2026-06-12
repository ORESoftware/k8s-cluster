# syntax=docker/dockerfile:1
FROM rust:1.90-bookworm AS build
ARG TARGETARCH
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    --mount=type=cache,target=/app/target,id=build-server-rs-target-${TARGETARCH},sharing=locked \
    cargo build --release \
 && cp target/release/dd-build-server /usr/local/bin/dd-build-server

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates git openssh-client \
  && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/dd-build-server /usr/local/bin/dd-build-server
ENTRYPOINT ["/usr/local/bin/dd-build-server"]
