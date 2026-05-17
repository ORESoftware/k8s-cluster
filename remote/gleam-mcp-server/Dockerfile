FROM ghcr.io/gleam-lang/gleam:v1.16.0-erlang-alpine
RUN apk add --no-cache postgresql-client

WORKDIR /app

# Build context is the repo root so the `dd_pg_defs` path dep in
# `remote/gleam-mcp-server/gleam.toml` resolves against the generated package.
COPY remote/gleam-mcp-server/gleam.toml ./remote/gleam-mcp-server/gleam.toml
COPY remote/gleam-mcp-server/manifest.toml ./remote/gleam-mcp-server/manifest.toml
COPY remote/gleam-mcp-server/src ./remote/gleam-mcp-server/src
COPY remote/libs/pg-defs/generated/gleam ./remote/libs/pg-defs/generated/gleam

WORKDIR /app/remote/gleam-mcp-server

EXPOSE 8090

CMD ["gleam", "run"]
