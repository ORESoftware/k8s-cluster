# syntax=docker/dockerfile:1
# Multi-stage build for canonical-backend.
FROM rust:1-slim-bookworm AS build
WORKDIR /build/canonical-backend.rs
COPY . .
RUN cargo build --release && strip target/release/canonical-backend

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build --chown=65532:65532 /build/canonical-backend.rs/target/release/canonical-backend /usr/local/bin/canonical-backend
# The built Astro site is mounted/copied in at deploy time.
ENV STATIC_DIR=/app/static
EXPOSE 8081
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/canonical-backend"]
