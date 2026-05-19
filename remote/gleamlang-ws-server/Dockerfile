FROM ghcr.io/gleam-lang/gleam:v1.16.0-erlang-alpine
RUN apk add --no-cache nodejs postgresql-client

WORKDIR /app

# Build context is the repo root so the `dd_pg_defs` path dep in
# `remote/gleamlang-ws-server/gleam.toml` resolves against the generated package.
COPY remote/gleamlang-ws-server/gleam.toml ./remote/gleamlang-ws-server/gleam.toml
COPY remote/gleamlang-ws-server/manifest.toml ./remote/gleamlang-ws-server/manifest.toml
COPY remote/gleamlang-ws-server/src ./remote/gleamlang-ws-server/src
COPY remote/gleamlang-ws-server/nats-bridge.mjs ./remote/gleamlang-ws-server/nats-bridge.mjs
COPY remote/gleamlang-ws-server/nats-client.mjs ./remote/gleamlang-ws-server/nats-client.mjs
COPY remote/libs/pg-defs/generated/gleam ./remote/libs/pg-defs/generated/gleam

WORKDIR /app/remote/gleamlang-ws-server

EXPOSE 8081

CMD ["gleam", "run"]
