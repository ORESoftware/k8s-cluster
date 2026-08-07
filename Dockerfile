# Multi-stage build for `dd-rust-network-mutex` (the live-mutex-rs broker).
#
# Stage 1: build the release binary against a pinned Rust toolchain.
#          Default features (`tls`, `otel`) are on so the resulting image
#          works as both a TLS-fronted broker and an OTel-instrumented
#          one out of the box. To opt out, override the build with
#          `--build-arg CARGO_BUILD_FLAGS="--no-default-features"`.
#
# Stage 2: minimal runtime image with only the binary and CA certs.
#          Runs as a non-root user (uid:gid 65532:65532) and exposes the
#          two listeners the broker uses: TCP 6970 (newline-delimited
#          JSON wire protocol) and HTTP 6971 (status page + serverless
#          callers + Prometheus `/metrics`).
#
# Build:
#   docker build -t oresoftware/live-mutex-rs:0.1.125 .
#
# Run:
#   docker run --rm -p 6970:6970 -p 6971:6971 \
#     oresoftware/live-mutex-rs:0.1.125
#
# See `readme.md` for the full env-var surface
# (`LMX_BIND_HOST`, `LMX_TCP_PORT`, `LMX_HTTP_PORT`, `LMX_AUTH_TOKEN`,
# `LMX_DEFAULT_TTL_MS`, etc.).

FROM rust:1.90-bookworm AS build

ARG CARGO_BUILD_FLAGS=""

WORKDIR /app

# Cache deps separately from src to keep rebuilds fast.
#
# Implementation note: we use a non-empty `src/lib.rs` (an inert
# `pub fn __stub() {}`) so the dependency-cache layer compiles a real
# library crate against the production Cargo.toml feature set. With an
# empty `lib.rs`, cargo's "fingerprint" caching has historically
# decided the dummy build's `target/release/dd-rust-network-mutex`
# binary is up-to-date even after the real `src/` is COPY'd in,
# producing a 343 KB stub binary in the runtime image instead of the
# real ~7 MB broker (this regressed twice; see live-mutex-rs#?).
#
# The matching belt-and-suspenders: `cargo clean -p` for the binary
# crate immediately before the real build, so cargo recompiles the
# binary even if its fingerprint somehow matches the dummy. Deps
# stay cached (only the package itself is invalidated), so this adds
# only a few seconds.
COPY Cargo.toml Cargo.lock build.rs ./
COPY vendor ./vendor
RUN mkdir -p src \
  && echo "fn main() {}" > src/main.rs \
  && printf 'pub fn __stub() {}\n' > src/lib.rs \
  && cargo build --release ${CARGO_BUILD_FLAGS} \
  && rm -rf src

COPY src ./src
COPY tests ./tests
COPY examples ./examples
COPY PROTOCOL.md ./PROTOCOL.md
COPY readme.md ./readme.md
COPY LICENSE ./LICENSE

RUN cargo clean -p dd-rust-network-mutex --release \
  && cargo build --release --bin dd-rust-network-mutex ${CARGO_BUILD_FLAGS}

# Sanity check inside the build stage — fail the build loudly if the
# binary is implausibly small (the stub was 343 KB; a real build of
# the broker is consistently 6–9 MB on Debian-bookworm rust:1.90).
RUN size_bytes=$(stat -c '%s' target/release/dd-rust-network-mutex) \
  && echo "built binary size: ${size_bytes} bytes" \
  && [ "${size_bytes}" -gt 2000000 ] || { \
       echo "ERROR: dd-rust-network-mutex is only ${size_bytes} bytes — likely the dependency-cache stub leaked into the runtime layer."; \
       exit 1; \
     }

FROM debian:bookworm-slim

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && apt-get clean \
  && rm -rf /var/lib/apt/lists/*

COPY --from=build /app/target/release/dd-rust-network-mutex /usr/local/bin/dd-rust-network-mutex
COPY lmx.toml /etc/dd-rust-network-mutex/lmx.toml
COPY .cli-flags.toml /etc/dd-rust-network-mutex/.cli-flags.toml

ENV LMX_BIND_HOST=0.0.0.0 \
    LMX_TCP_PORT=6970 \
    LMX_HTTP_PORT=6971 \
    LMX_CONFIG=/etc/dd-rust-network-mutex/lmx.toml \
    LMX_CLI_FLAGS_CONFIG=/etc/dd-rust-network-mutex/.cli-flags.toml \
    LMX_LOG_FORMAT=text \
    RUST_LOG=info,lmx=info

EXPOSE 6970 6971

USER 65532:65532

ENTRYPOINT ["/usr/local/bin/dd-rust-network-mutex"]
CMD []
