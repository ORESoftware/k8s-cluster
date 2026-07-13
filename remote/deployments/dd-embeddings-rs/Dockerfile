# syntax=docker/dockerfile:1
# Multi-stage build for dd-embeddings-rs.
#
# Stage 1: build the release binary against a pinned Rust toolchain.
# Stage 2: minimal runtime image with the binary + CA certs (we make outbound
#          TLS calls to OpenAI, Voyage, Google, Cohere, etc. and to Qdrant).

FROM rust:1.95-bookworm AS build
ARG TARGETARCH

WORKDIR /app

# Dependency downloads + compiled artifacts are cached across builds via the
# BuildKit cargo cache mounts on the build RUN below.
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git,sharing=locked \
    --mount=type=cache,target=/app/target,id=dd-embeddings-rs-target-${TARGETARCH},sharing=locked \
    cargo build --release --bin dd-embeddings-rs \
 && cp target/release/dd-embeddings-rs /usr/local/bin/dd-embeddings-rs

FROM debian:bookworm-slim

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && apt-get clean \
  && rm -rf /var/lib/apt/lists/*

COPY --from=build /usr/local/bin/dd-embeddings-rs /usr/local/bin/dd-embeddings-rs

ENV EMBEDDINGS_HOST=0.0.0.0
ENV EMBEDDINGS_PORT=8090

EXPOSE 8090

USER 65532:65532

CMD ["/usr/local/bin/dd-embeddings-rs"]
