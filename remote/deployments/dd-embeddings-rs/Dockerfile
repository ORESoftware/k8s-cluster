# Multi-stage build for dd-embeddings-rs.
#
# Stage 1: build the release binary against a pinned Rust toolchain.
# Stage 2: minimal runtime image with the binary + CA certs (we make outbound
#          TLS calls to OpenAI, Voyage, Google, Cohere, etc. and to Qdrant).

FROM rust:1.95-bookworm AS build

WORKDIR /app

# Cache deps separately from src to keep rebuilds fast.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs \
  && cargo build --release \
  && rm -rf src

COPY src ./src
RUN cargo build --release --bin dd-embeddings-rs

FROM debian:bookworm-slim

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && apt-get clean \
  && rm -rf /var/lib/apt/lists/*

COPY --from=build /app/target/release/dd-embeddings-rs /usr/local/bin/dd-embeddings-rs

ENV EMBEDDINGS_HOST=0.0.0.0
ENV EMBEDDINGS_PORT=8090

EXPOSE 8090

USER 65532:65532

CMD ["/usr/local/bin/dd-embeddings-rs"]
