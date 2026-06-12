FROM ghcr.io/gleam-lang/gleam:v1.16.0-erlang-alpine
# The OpenTelemetry exporter transitively pulls in ts_chatterbox, which sets
# `warnings_as_errors` and uses a now-deprecated `catch ...` form that Erlang/OTP
# 29 promotes to a hard error. Suppressing that one deprecation warning keeps the
# dependency compiling; it is a no-op on older OTP releases.
ENV ERL_COMPILER_OPTIONS="[nowarn_deprecated_catch]"
RUN apk add --no-cache nodejs postgresql-client

WORKDIR /app

# Build context is the repo root so the `dd_pg_defs` path dep in
# `remote/deployments/gleamlang-ws-server/gleam.toml` resolves against the generated package.
COPY remote/deployments/gleamlang-ws-server/gleam.toml ./remote/deployments/gleamlang-ws-server/gleam.toml
COPY remote/deployments/gleamlang-ws-server/manifest.toml ./remote/deployments/gleamlang-ws-server/manifest.toml
COPY remote/deployments/gleamlang-ws-server/src ./remote/deployments/gleamlang-ws-server/src
COPY remote/deployments/gleamlang-ws-server/nats-bridge.mjs ./remote/deployments/gleamlang-ws-server/nats-bridge.mjs
COPY remote/deployments/gleamlang-ws-server/nats-client.mjs ./remote/deployments/gleamlang-ws-server/nats-client.mjs
COPY remote/libs/pg-defs/generated/gleam ./remote/libs/pg-defs/generated/gleam
COPY remote/libs/otel-client-gleam ./remote/libs/otel-client-gleam

WORKDIR /app/remote/deployments/gleamlang-ws-server

EXPOSE 8081

CMD ["gleam", "run"]
