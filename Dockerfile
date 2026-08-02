# syntax=docker/dockerfile:1
# Multi-stage build for fiducia-backend.
FROM node:26-slim@sha256:715e55e4b84e4bb0ff48e49b398a848f08e55daed8eb6a0ea1839ae53bc57583 AS web
RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates
ARG MARKETING_REF=f3fd0e63b3c0898bed33c66817847056067f7ca8
ARG INTERFACES_SHA=6081bc3f3b7cbe0312870968b61acc38ca91c66a
ARG TEST_CONFIG_REF=026fcf28193c7baeb7f5feb68480a91539c1f0fa
WORKDIR /web
RUN for value in "$MARKETING_REF" "$INTERFACES_SHA" "$TEST_CONFIG_REF"; do \
      test "${#value}" -eq 40 \
      && test -z "$(printf '%s' "$value" | tr -d '0-9a-f')"; \
    done \
    && git init --quiet fiducia-marketing.web \
    && git -C fiducia-marketing.web remote add origin https://github.com/fiducia-cloud/fiducia-marketing.web.git \
    && git -C fiducia-marketing.web fetch --quiet --depth=1 --no-tags origin "$MARKETING_REF" \
    && git -C fiducia-marketing.web checkout --quiet --detach FETCH_HEAD \
    && test "$(git -C fiducia-marketing.web rev-parse HEAD)" = "$MARKETING_REF" \
    && git init --quiet fiducia-interfaces \
    && git -C fiducia-interfaces remote add origin https://github.com/fiducia-cloud/fiducia-interfaces.git \
    && git -C fiducia-interfaces fetch --quiet --depth=1 --no-tags origin "$INTERFACES_SHA" \
    && git -C fiducia-interfaces checkout --quiet --detach FETCH_HEAD \
    && test "$(git -C fiducia-interfaces rev-parse HEAD)" = "$INTERFACES_SHA" \
    && git init --quiet fiducia-test-config \
    && git -C fiducia-test-config remote add origin https://github.com/fiducia-cloud/fiducia-test-config.git \
    && git -C fiducia-test-config fetch --quiet --depth=1 --no-tags origin "$TEST_CONFIG_REF" \
    && git -C fiducia-test-config checkout --quiet --detach FETCH_HEAD \
    && test "$(git -C fiducia-test-config rev-parse HEAD)" = "$TEST_CONFIG_REF"
WORKDIR /web/fiducia-marketing.web
RUN npm ci --ignore-scripts \
    && PUBLIC_BASE=/ npm run build

FROM rust:1.97.1-slim-bookworm@sha256:99e09cb2284e2ddbb73a995deee3e91783fd04d177602ccf6eab326d778ee777 AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates
WORKDIR /build
# Immutable cross-repository input. Bump this SHA together with the CI checkout.
ARG INTERFACES_SHA=6081bc3f3b7cbe0312870968b61acc38ca91c66a
ARG PAYMENTS_REF=126dc5d1b3fcba289f95139284dc0f3c54b56a8a
RUN for value in "$INTERFACES_SHA" "$PAYMENTS_REF"; do \
      test "${#value}" -eq 40 \
      && test -z "$(printf '%s' "$value" | tr -d '0-9a-f')"; \
    done \
    && git init fiducia-interfaces \
    && git -C fiducia-interfaces remote add origin \
       https://github.com/fiducia-cloud/fiducia-interfaces.git \
    && git -C fiducia-interfaces fetch --depth 1 origin "$INTERFACES_SHA" \
    && git -C fiducia-interfaces checkout --detach FETCH_HEAD \
    && test "$(git -C fiducia-interfaces rev-parse HEAD)" = "$INTERFACES_SHA" \
    && git init fiducia-payments.rs \
    && git -C fiducia-payments.rs remote add origin \
       https://github.com/fiducia-cloud/fiducia-payments.rs.git \
    && git -C fiducia-payments.rs fetch --depth 1 origin "$PAYMENTS_REF" \
    && git -C fiducia-payments.rs checkout --detach FETCH_HEAD \
    && test "$(git -C fiducia-payments.rs rev-parse HEAD)" = "$PAYMENTS_REF"
COPY . fiducia-customer.rs
WORKDIR /build/fiducia-customer.rs
RUN cargo build --locked --release && strip target/release/fiducia-backend

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:fccdbb0a547c14e23fcf4ce8ad62ca5d43b4faae8d22cd292f490fef9946c96e
COPY --from=build --chown=65532:65532 /build/fiducia-customer.rs/target/release/fiducia-backend /usr/local/bin/fiducia-backend
COPY --from=web --chown=65532:65532 /web/fiducia-marketing.web/dist /app/static
ENV STATIC_DIR=/app/static
EXPOSE 8080
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/fiducia-backend"]
