# dd-ocr-rs ships as a built image (not the cargo-run-from-hostPath pattern some
# sibling dd-*-rs services use) because it compiles + links a native OCR bridge
# at build time: the Tesseract C++ API + Leptonica (cpp/tesseract_bridge.cpp).
#
# IMPORTANT: build from the cluster repo root so the Cargo path-dependencies
# under libs/ are in the build context:
#
#   docker build -f deployments/dd-ocr-rs/Dockerfile -t dd-ocr-rs .
#
# (The repo-root .dockerignore keeps the context to just this crate + its libs.)

# syntax=docker/dockerfile:1

# ---------------------------------------------------------------------------
# 1) Build the Rust service: compile the Tesseract C++ shim + link it against
#    libtesseract + Leptonica. The repo layout is replicated under /src so the
#    crate's ../../libs path dependencies resolve.
# ---------------------------------------------------------------------------
FROM rust:1.90-bookworm AS rust-build
WORKDIR /src
# build.rs uses pkg-config to discover Tesseract + Leptonica; the -dev packages
# provide headers (tesseract/baseapi.h, leptonica/allheaders.h), pkg-config
# files, and the C++ libraries to compile/link the shim against.
RUN apt-get update \
  && apt-get install -y --no-install-recommends \
       pkg-config libtesseract-dev libleptonica-dev \
  && rm -rf /var/lib/apt/lists/*
# Cargo path-dependencies (and their transitive ones).
COPY libs/nats/subject-defs/generated/rust ./libs/nats/subject-defs/generated/rust
COPY libs/runtime-config-client-rs ./libs/runtime-config-client-rs
COPY libs/interfaces/shared/generated/rust ./libs/interfaces/shared/generated/rust
COPY deployments/dd-ocr-rs ./deployments/dd-ocr-rs
WORKDIR /src/deployments/dd-ocr-rs
RUN cargo build --release --locked && strip target/release/dd-ocr-rs

# ---------------------------------------------------------------------------
# 2) Runtime image. Ships the Tesseract + Leptonica shared libraries and the
#    English trained data; add more languages with extra tesseract-ocr-* pkgs.
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim
# Runtime needs only the shared libraries + trained data, never the -dev headers.
# libtesseract5 pulls the matching Leptonica runtime in transitively, and
# tesseract-ocr-eng provides eng.traineddata.
RUN apt-get update \
  && apt-get install -y --no-install-recommends \
       ca-certificates \
       libtesseract5 tesseract-ocr-eng \
  && rm -rf /var/lib/apt/lists/*
# Where the tesseract-ocr-eng package drops eng.traineddata.
ENV TESSDATA_PREFIX=/usr/share/tesseract-ocr/5/tessdata
COPY --from=rust-build /src/deployments/dd-ocr-rs/target/release/dd-ocr-rs \
     /usr/local/bin/dd-ocr-rs
EXPOSE 8123
ENTRYPOINT ["/usr/local/bin/dd-ocr-rs"]
