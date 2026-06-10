# dd-document-rs ships as a built image (not the cargo-run-from-hostPath pattern
# the sibling dd-*-rs services use) because it links two native bridges at build
# time: the Pandoc Haskell SDK and the ImageMagick C++ SDK (Magick++).
#
# IMPORTANT: build from the cluster repo root so the Cargo path-dependencies
# under libs/ are in the build context:
#
#   docker build -f deployments/dd-document-rs/Dockerfile -t dd-document-rs .
#
# (The repo-root .dockerignore keeps the context to just this crate + its libs.)

# syntax=docker/dockerfile:1

# ---------------------------------------------------------------------------
# 1) Build the Pandoc Haskell bridge as a standalone shared library.
#    `standalone` statically bundles the GHC RTS + Haskell deps into one .so,
#    leaving only system C libs (gmp, ffi, zlib, numa) as runtime deps.
# ---------------------------------------------------------------------------
FROM haskell:9.6 AS haskell-bridge
WORKDIR /bridge
# Build dependencies first (keyed on the cabal files only) so editing the bridge
# source doesn't trigger a full ~100-package pandoc rebuild on every iteration.
COPY deployments/dd-document-rs/haskell/dd-pandoc-bridge.cabal \
     deployments/dd-document-rs/haskell/cabal.project ./
RUN cabal update && cabal build --only-dependencies
COPY deployments/dd-document-rs/haskell/src ./src
RUN cabal build dd-pandoc-bridge
# Collect the produced shared object, normalize its name, and strip it (~345MB
# -> ~234MB) since the final image only needs the runtime symbols.
RUN set -eux; \
    mkdir -p /out; \
    so="$(find dist-newstyle -name 'libdd-pandoc-bridge*.so*' | head -n1)"; \
    test -n "$so"; \
    cp -av "$so" /out/libdd-pandoc-bridge.so; \
    strip --strip-unneeded /out/libdd-pandoc-bridge.so

# ---------------------------------------------------------------------------
# 2) Build the Rust service: compile the Magick++ shim + link both bridges.
#    The repo layout is replicated under /src so the crate's ../../libs path
#    dependencies resolve.
# ---------------------------------------------------------------------------
FROM rust:1.90-bookworm AS rust-build
WORKDIR /src
ENV PANDOC_BRIDGE_LIB_DIR=/usr/local/lib/dd-document-rs
# build.rs uses pkg-config to discover Magick++; libmagick++-dev provides headers,
# pkg-config files, and the C++ library to compile/link the shim against.
RUN apt-get update \
  && apt-get install -y --no-install-recommends pkg-config libmagick++-dev \
  && rm -rf /var/lib/apt/lists/*
COPY --from=haskell-bridge /out/ ${PANDOC_BRIDGE_LIB_DIR}/
# Cargo path-dependencies (and their transitive ones).
COPY libs/nats/subject-defs/generated/rust ./libs/nats/subject-defs/generated/rust
COPY libs/runtime-config-client-rs ./libs/runtime-config-client-rs
COPY libs/interfaces/shared/generated/rust ./libs/interfaces/shared/generated/rust
COPY deployments/dd-document-rs ./deployments/dd-document-rs
WORKDIR /src/deployments/dd-document-rs
RUN cargo build --release --locked && strip target/release/dd-document-rs

# ---------------------------------------------------------------------------
# 3) Runtime image.
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends \
       ca-certificates libgmp10 libffi8 zlib1g libnuma1 \
       libmagick++-6.q16-8 \
  && rm -rf /var/lib/apt/lists/*

# Opt-in PDF engine. `--build-arg PDF_ENGINE=typst` installs the self-contained
# typst binary (~30MB, no TeX, no network at runtime). Default image has no PDF
# engine and /convert-pdf returns 503.
ARG PDF_ENGINE=none
ARG TYPST_VERSION=0.12.0
RUN if [ "$PDF_ENGINE" = "typst" ]; then \
      set -eux; \
      apt-get update && apt-get install -y --no-install-recommends curl xz-utils; \
      arch="$(uname -m)"; \
      url="https://github.com/typst/typst/releases/download/v${TYPST_VERSION}/typst-${arch}-unknown-linux-musl.tar.xz"; \
      curl -fsSL "$url" -o /tmp/typst.tar.xz; \
      tar -xJf /tmp/typst.tar.xz -C /tmp; \
      install -m 0755 "/tmp/typst-${arch}-unknown-linux-musl/typst" /usr/local/bin/typst; \
      apt-get purge -y curl xz-utils && apt-get autoremove -y; \
      rm -rf /tmp/typst* /var/lib/apt/lists/*; \
      /usr/local/bin/typst --version; \
    fi
# Install the hardening policy so dangerous ImageMagick coders (URL/MVG/MSL/PDF/
# PS/SVG/...) are disabled and resource limits apply process-wide.
ENV MAGICK_CONFIGURE_PATH=/etc/dd-imagemagick
COPY deployments/dd-document-rs/cpp/policy.xml /etc/dd-imagemagick/policy.xml
ENV LD_LIBRARY_PATH=/usr/local/lib/dd-document-rs
COPY --from=haskell-bridge /out/ /usr/local/lib/dd-document-rs/
COPY --from=rust-build /src/deployments/dd-document-rs/target/release/dd-document-rs \
     /usr/local/bin/dd-document-rs
EXPOSE 8122
ENTRYPOINT ["/usr/local/bin/dd-document-rs"]
