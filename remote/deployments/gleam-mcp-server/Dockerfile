FROM ghcr.io/gleam-lang/gleam:v1.16.0-erlang-alpine
# The OpenTelemetry exporter transitively pulls in ts_chatterbox, which sets
# `warnings_as_errors` and uses a now-deprecated `catch ...` form that Erlang/OTP
# 29 promotes to a hard error. Suppressing that one deprecation warning keeps the
# dependency compiling; it is a no-op on older OTP releases.
ENV ERL_COMPILER_OPTIONS="[nowarn_deprecated_catch]"
RUN apk add --no-cache postgresql-client

WORKDIR /app

# Build context is the repo root so the path deps in
# `remote/deployments/gleam-mcp-server/gleam.toml` resolve against the generated
# packages. Every local-path dep must be copied or `gleam run` fails to resolve
# it (the NetworkPolicy blocks hex.pm at runtime anyway).
COPY remote/deployments/gleam-mcp-server/gleam.toml ./remote/deployments/gleam-mcp-server/gleam.toml
COPY remote/deployments/gleam-mcp-server/manifest.toml ./remote/deployments/gleam-mcp-server/manifest.toml
COPY remote/deployments/gleam-mcp-server/src ./remote/deployments/gleam-mcp-server/src
COPY remote/libs/pg-defs/generated/gleam ./remote/libs/pg-defs/generated/gleam
COPY remote/libs/runtime-config-client-gleam ./remote/libs/runtime-config-client-gleam
COPY remote/libs/nats/subject-defs/generated/gleam ./remote/libs/nats/subject-defs/generated/gleam
COPY remote/libs/otel-client-gleam ./remote/libs/otel-client-gleam

WORKDIR /app/remote/deployments/gleam-mcp-server

EXPOSE 8090

CMD ["gleam", "run"]
