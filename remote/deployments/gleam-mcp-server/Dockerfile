FROM ghcr.io/gleam-lang/gleam:v1.16.0-erlang-alpine
RUN apk add --no-cache postgresql-client

WORKDIR /app

# Build context is the repo root so the `dd_pg_defs` path dep in
# `remote/deployments/gleam-mcp-server/gleam.toml` resolves against the generated package.
COPY remote/deployments/gleam-mcp-server/gleam.toml ./remote/deployments/gleam-mcp-server/gleam.toml
COPY remote/deployments/gleam-mcp-server/manifest.toml ./remote/deployments/gleam-mcp-server/manifest.toml
COPY remote/deployments/gleam-mcp-server/src ./remote/deployments/gleam-mcp-server/src
COPY remote/libs/pg-defs/generated/gleam ./remote/libs/pg-defs/generated/gleam

WORKDIR /app/remote/deployments/gleam-mcp-server

EXPOSE 8090

CMD ["gleam", "run"]
