FROM ghcr.io/gleam-lang/gleam:v1.16.0-erlang-alpine
RUN apk add --no-cache nodejs postgresql-client

WORKDIR /app

# Build context is the repo root so the `dd_pg_defs` path dep in
# `remote/deployments/gleamlang-server/gleam.toml` resolves against the generated package.
COPY remote/deployments/gleamlang-server/gleam.toml ./remote/deployments/gleamlang-server/gleam.toml
COPY remote/deployments/gleamlang-server/manifest.toml ./remote/deployments/gleamlang-server/manifest.toml
COPY remote/deployments/gleamlang-server/src ./remote/deployments/gleamlang-server/src
COPY remote/deployments/gleamlang-server/nats-bridge.mjs ./remote/deployments/gleamlang-server/nats-bridge.mjs
COPY remote/deployments/gleamlang-server/nats-client.mjs ./remote/deployments/gleamlang-server/nats-client.mjs
COPY remote/libs/pg-defs/generated/gleam ./remote/libs/pg-defs/generated/gleam

WORKDIR /app/remote/deployments/gleamlang-server

EXPOSE 8081

CMD ["gleam", "run"]
