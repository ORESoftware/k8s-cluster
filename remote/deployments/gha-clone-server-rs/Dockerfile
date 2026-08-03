# syntax=docker/dockerfile:1
FROM rust:1.90-bookworm AS build
WORKDIR /src
COPY Cargo.toml build.rs ./
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=gha-clone-cargo-registry,sharing=locked \
    --mount=type=cache,target=/src/target,id=gha-clone-target,sharing=locked \
    cargo test --release \
 && cargo build --release \
 && cp target/release/gha-clone-server /usr/local/bin/gha-clone-server

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --create-home gha-clone
COPY --from=build /usr/local/bin/gha-clone-server /usr/local/bin/gha-clone-server
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/gha-clone-server"]
