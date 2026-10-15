# syntax=docker/dockerfile:1.7

# Layer-cached, multi-stage build for dd-sound-recorder-rs.
#
#   docker build -t sonus-auris-backend:dev .
#
# The expensive part of a Rust build is compiling the dependency tree
# (aws-lc-sys, ring, aws-sdk-s3, …) — ~12-13 min cold. cargo-chef splits that into
# its own Docker layer keyed only on Cargo.toml/Cargo.lock, so a *source-only*
# change reuses the cached dependency layer and recompiles just this crate
# (~1-2 min). That dependency layer is part of the image, so it also survives
# across build hosts / CI when pushed to a registry — unlike a BuildKit
# --mount=type=cache, which is host-local and never shipped.

############################
# Stage 0 — toolchain + cargo-chef
############################
FROM rust:1.91.1-bookworm AS chef
WORKDIR /app
# Install cargo-chef once; this layer is reused until the base image changes.
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    cargo install cargo-chef --locked

############################
# Stage 1 — plan: derive the dependency recipe (depends only on the manifests)
############################
FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY generated ./generated
COPY src ./src
RUN cargo chef prepare --recipe-path recipe.json

############################
# Stage 2 — build: cook deps (CACHED LAYER), then compile the app
############################
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# (1) Compile ONLY dependencies into /app/target. Because /app/target is NOT a
#     cache mount here, the compiled deps are baked into this image layer and are
#     reused on every build whose recipe.json (i.e. Cargo.toml/Cargo.lock) is
#     unchanged. The registry/git mounts only speed crate *downloads*.
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    cargo chef cook --release --recipe-path recipe.json
# (2) Bring in the real source and build just the application crate. The cooked
#     dependency artifacts in /app/target are reused, so this step is fast.
COPY Cargo.toml Cargo.lock ./
COPY generated ./generated
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    cargo build --release --locked --bin dd-sound-recorder-rs \
 && cp target/release/dd-sound-recorder-rs /usr/local/bin/dd-sound-recorder-rs

############################
# Stage 3 — runtime: slim, non-root, just the binary + TLS roots
############################
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && apt-get clean && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/local/bin/dd-sound-recorder-rs /usr/local/bin/dd-sound-recorder-rs

ENV HOST=0.0.0.0 \
    PORT=8126
EXPOSE 8126
USER 10001:10001
CMD ["/usr/local/bin/dd-sound-recorder-rs"]
