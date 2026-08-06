# syntax=docker/dockerfile:1

FROM node:26-bookworm-slim@sha256:2d49d876e96237d76de412761cf05dbfe5aee325cc4406a4d41d5824c5bb8beb AS client-build
WORKDIR /build/client
COPY client/package.json client/package-lock.json ./
RUN npm ci
COPY client/ ./
RUN npm run typecheck && npm test && npm run build

FROM rust:1.97-slim-bookworm@sha256:99e09cb2284e2ddbb73a995deee3e91783fd04d177602ccf6eab326d778ee777 AS rust-base
RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    apt-get update \
    && apt-get install --yes --no-install-recommends build-essential cmake
WORKDIR /build/canonical-web-server.rs

# The no-ingress worker build intentionally has no dependency on the browser
# bundle or the customer HTTP binary.
FROM rust-base AS revoker-build
COPY . .
RUN cargo build --locked --release -p canonical-session-revoker \
    && strip target/release/canonical-session-revoker

# The API image is intentionally independent of the browser bundle. It serves
# only the REST and WebSocket route family used by api.canonical.plus.
FROM rust-base AS api-build
COPY . .
RUN cargo build --locked --release -p canonical-web-server --bin canonical-api-server \
    && strip target/release/canonical-api-server

FROM rust-base AS web-build
COPY . .
COPY --from=client-build /build/client/dist ./client/dist
RUN cargo build --locked --release -p canonical-web-server --bin canonical-web-server \
    && strip target/release/canonical-web-server

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:fccdbb0a547c14e23fcf4ce8ad62ca5d43b4faae8d22cd292f490fef9946c96e AS revoker
COPY --from=revoker-build --chown=65532:65532 \
    /build/canonical-web-server.rs/target/release/canonical-session-revoker \
    /usr/local/bin/canonical-session-revoker
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/canonical-session-revoker"]

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:fccdbb0a547c14e23fcf4ce8ad62ca5d43b4faae8d22cd292f490fef9946c96e AS api
COPY --from=api-build --chown=65532:65532 \
    /build/canonical-web-server.rs/target/release/canonical-api-server \
    /usr/local/bin/canonical-api-server
EXPOSE 8081
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/canonical-api-server"]

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:fccdbb0a547c14e23fcf4ce8ad62ca5d43b4faae8d22cd292f490fef9946c96e AS web
COPY --from=web-build --chown=65532:65532 \
    /build/canonical-web-server.rs/target/release/canonical-web-server \
    /usr/local/bin/canonical-web-server
COPY --from=client-build --chown=65532:65532 /build/client/dist /app/client
ENV APP_ASSET_DIR=/app/client
ENV STATIC_DIR=/app/static
EXPOSE 8081
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/canonical-web-server"]
