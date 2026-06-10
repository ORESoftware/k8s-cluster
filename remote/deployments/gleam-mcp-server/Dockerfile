FROM ghcr.io/gleam-lang/gleam:v1.16.0-erlang-alpine
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

WORKDIR /app/remote/deployments/gleam-mcp-server

EXPOSE 8090

CMD ["gleam", "run"]
