# syntax=docker/dockerfile:1.7

# Layer-cached, multi-stage build for the t2v-v2t.rs workspace.
#
#   docker build --build-arg BIN=t2v-api -t t2v-api:dev .
#   docker build --build-arg BIN=t2v-web -t t2v-web:dev .
#
# The workspace has two binaries (t2v-api, t2v-web) that deploy separately;
# BIN selects which one this image runs. cargo-chef splits the expensive
# dependency compile (sea-orm, reqwest+rustls, tokio) into its own layer keyed
# only on the Cargo manifests, so a source-only change reuses the cached
# dependency layer. That layer is baked into the image (not a host-local
# BuildKit cache), so it survives across build hosts and CI.

ARG BIN=t2v-api

############################
# Stage 0 — toolchain + cargo-chef
############################
FROM rust:1.91.1-bookworm AS chef
WORKDIR /app
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    cargo install cargo-chef --locked

############################
# Stage 1 — plan: derive the dependency recipe from the manifests
############################
FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo chef prepare --recipe-path recipe.json

############################
# Stage 2 — build: cook deps (CACHED), then compile the selected binary
############################
FROM chef AS builder
ARG BIN
COPY --from=planner /app/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    cargo chef cook --release --recipe-path recipe.json
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    cargo build --release --locked --bin "$BIN" \
 && cp "target/release/$BIN" /usr/local/bin/t2v-service

############################
# Stage 3 — runtime: slim, non-root, just the binary + TLS roots
############################
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --uid 1000 --user-group --home-dir /home/t2v --create-home t2v
COPY --from=builder /usr/local/bin/t2v-service /usr/local/bin/t2v-service
USER 1000:1000
# t2v-api defaults to 8130, t2v-web to 8131; both honor $PORT.
EXPOSE 8130 8131
ENTRYPOINT ["/usr/local/bin/t2v-service"]
