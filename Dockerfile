# Multi-stage build for `tor-server`.
#
# Stage 1 compiles the release binary against a pinned toolchain, caching the
# dependency layer separately from the source for fast rebuilds.
# Stage 2 is a minimal Debian-slim runtime with only the binary + CA certs,
# running as a non-root user.
#
# Build:  docker build -t oresoftware/tor-server:0.1.0 .
# Run (relay):
#   docker run --rm -p 9001:9001 -e TOR_ROLE=relay \
#     -e TOR_LISTEN=0.0.0.0:9001 -v tor-keys:/data \
#     -e TOR_KEY_FILE=/data/relay.key oresoftware/tor-server:0.1.0
# Run (client / SOCKS5):
#   docker run --rm -p 9050:9050 -e TOR_ROLE=client \
#     -e TOR_SOCKS_LISTEN=0.0.0.0:9050 -e TOR_DIRECTORY=/etc/tor/directory.toml \
#     -v $PWD/directory.toml:/etc/tor/directory.toml oresoftware/tor-server:0.1.0

FROM rust:1.90-bookworm AS build
WORKDIR /app

# Cache dependencies against a stub source tree first.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src \
  && echo "fn main() {}" > src/main.rs \
  && cargo build --release \
  && rm -rf src

COPY src ./src
# Ensure the real binary is rebuilt even if the stub fingerprint matches.
RUN cargo clean -p tor-server --release && cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/* \
  && useradd --system --uid 65532 --user-group --no-create-home tor \
  && mkdir -p /data && chown 65532:65532 /data
COPY --from=build /app/target/release/tor-server /usr/local/bin/tor-server
COPY docs /docs
USER 65532:65532
WORKDIR /data
ENV TOR_ROLE=relay TOR_LISTEN=0.0.0.0:9001 TOR_KEY_FILE=/data/relay.key \
    TOR_SOCKS_LISTEN=0.0.0.0:9050 TOR_UI_LISTEN=0.0.0.0:9060 TOR_DOCS_DIR=/docs
EXPOSE 9001 9050 9060
ENTRYPOINT ["/usr/local/bin/tor-server"]
