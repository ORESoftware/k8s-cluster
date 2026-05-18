# syntax=docker/dockerfile:1.6

# Multi-stage Dockerfile for gleamlang-presence-server.
#
# We compile the Gleam project + Erlang release in a fat builder image and
# copy the resulting release into a slim runtime image. The runtime image
# uses the same OTP minor version as the build to avoid VSN mismatches.

# ──────────────────────────────────────────────────────────────────────────
# Stage 1: build the Erlang release
# ──────────────────────────────────────────────────────────────────────────
FROM ghcr.io/gleam-lang/gleam:v1.13.0-erlang-alpine AS builder

# Build-time deps: rebar3 for release packaging, build essentials for
# native NIFs that pog/pgo pull in.
RUN apk add --no-cache build-base git

WORKDIR /build

# Copy the dependency lock first so layer caching kicks in.
COPY gleam.toml manifest.toml ./
COPY src ./src

# The sibling pg-defs adapter is a path dependency. Layer it in too.
# The path declared in gleam.toml is `../libs/pg-defs/generated/gleam`, so
# the build context must contain `remote/libs/pg-defs/generated/gleam`.
# When building from `remote/`:
#   docker build -f gleamlang-presence-server/Dockerfile -t presence:0.1.0 .
COPY ../libs/pg-defs/generated/gleam /libs/pg-defs/generated/gleam

# Symlink so the path in gleam.toml resolves inside the builder layout.
RUN mkdir -p /build/../libs && ln -sf /libs/pg-defs /build/../libs/pg-defs

RUN gleam deps download
RUN gleam export erlang-shipment

# ──────────────────────────────────────────────────────────────────────────
# Stage 2: minimal runtime
# ──────────────────────────────────────────────────────────────────────────
FROM erlang:27-alpine AS runtime

RUN apk add --no-cache libstdc++ ncurses-libs openssl ca-certificates

# Non-root user.
RUN addgroup -S app && adduser -S -G app app
WORKDIR /app

COPY --from=builder /build/build/erlang-shipment ./
RUN chown -R app:app /app

USER app

EXPOSE 8081 4369 9100

# Environment knobs (override at runtime):
#   PORT                          — HTTP/WS listen port            (default 8081)
#   PG_DATABASE_URL               — postgres://... (optional)      (default in-memory)
#   CLUSTER_NAMESPACE             — k8s namespace                  (default "default")
#   CLUSTER_LABEL_SELECTOR        — k8s pod label selector         (default "app=presence")
#   CLUSTER_NODE_PREFIX           — short erlang node name prefix  (default "presence")
#   CLUSTER_HEADLESS_SERVICE      — k8s headless service name      (default "presence-svc")
#   CLUSTER_DISCOVERY_INTERVAL_MS — discovery loop interval        (default 5000)
#   RELEASE_NODE                  — full long-name (set by k8s)
#   RELEASE_COOKIE                — erlang cookie (set by k8s Secret)
#   ERL_AFLAGS                    — typically "-kernel inet_dist_listen_min 9100 inet_dist_listen_max 9100"

ENV PORT=8081 \
    ERL_AFLAGS="-kernel inet_dist_listen_min 9100 inet_dist_listen_max 9100"

ENTRYPOINT ["./entrypoint.sh", "run"]
