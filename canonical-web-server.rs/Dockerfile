# syntax=docker/dockerfile:1

FROM node:26-bookworm-slim AS client-build
WORKDIR /build/client
COPY client/package.json client/package-lock.json ./
RUN npm ci
COPY client/ ./
RUN npm run typecheck && npm test && npm run build

FROM rust:1.97-slim-bookworm AS rust-build
RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    apt-get update \
    && apt-get install --yes --no-install-recommends build-essential cmake
WORKDIR /build/canonical-web-server.rs
COPY . .
COPY --from=client-build /build/client/dist ./client/dist
RUN cargo build --locked --release \
    && strip target/release/canonical-web-server

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=rust-build --chown=65532:65532 \
    /build/canonical-web-server.rs/target/release/canonical-web-server \
    /usr/local/bin/canonical-web-server
COPY --from=client-build --chown=65532:65532 /build/client/dist /app/client
ENV APP_ASSET_DIR=/app/client
ENV STATIC_DIR=/app/static
EXPOSE 8081
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/canonical-web-server"]
