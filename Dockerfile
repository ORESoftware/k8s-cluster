# syntax=docker/dockerfile:1.7

FROM ghcr.io/gleam-lang/gleam:v1.16.0-erlang-alpine AS builder
ENV ERL_COMPILER_OPTIONS="[nowarn_deprecated_catch]"
RUN apk add --no-cache build-base git rebar3
WORKDIR /build/remote/deployments/scintilla-run-monorepo/apps/gleam-lambda-runner

COPY remote/deployments/scintilla-run-monorepo/apps/gleam-lambda-runner/gleam.toml ./
COPY remote/deployments/scintilla-run-monorepo/apps/gleam-lambda-runner/manifest.toml ./
COPY remote/deployments/scintilla-run-monorepo/apps/gleam-lambda-runner/src ./src
COPY remote/libs/nats/subject-defs/generated/gleam /build/remote/libs/nats/subject-defs/generated/gleam
COPY remote/libs/cli-config-client-gleam /build/remote/libs/cli-config-client-gleam
COPY remote/libs/pg-defs/generated/gleam /build/remote/libs/pg-defs/generated/gleam
COPY remote/libs/otel-client-gleam /build/remote/libs/otel-client-gleam
COPY remote/libs/runtime-config-client-gleam /build/remote/libs/runtime-config-client-gleam

RUN gleam deps download \
  && gleam export erlang-shipment

FROM docker.io/library/erlang:28-alpine AS runtime
RUN apk add --no-cache \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/main \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/community \
  bash \
  ca-certificates \
  libstdc++ \
  ncurses-libs \
  nodejs-current \
  openssl \
  postgresql-client \
  python3 \
  ruby
WORKDIR /app
COPY --from=builder /build/remote/deployments/scintilla-run-monorepo/apps/gleam-lambda-runner/build/erlang-shipment ./
COPY remote/deployments/scintilla-run-monorepo/apps/gleam-lambda-runner/child-runtimes /app/remote/deployments/scintilla-run-monorepo/apps/gleam-lambda-runner/child-runtimes
COPY remote/libs/nats/subject-defs/generated/javascript /app/remote/libs/nats/subject-defs/generated/javascript
WORKDIR /app/remote/deployments/scintilla-run-monorepo/apps/gleam-lambda-runner
ENV ERL_COMPILER_OPTIONS="[nowarn_deprecated_catch]"
EXPOSE 8083
ENTRYPOINT ["/app/entrypoint.sh", "run"]
