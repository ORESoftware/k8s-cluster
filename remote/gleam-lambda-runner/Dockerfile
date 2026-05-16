FROM ghcr.io/gleam-lang/gleam:v1.16.0-erlang-alpine
RUN apk add --no-cache \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/community \
  nodejs-current \
  postgresql-client
WORKDIR /app
COPY remote/gleam-lambda-runner/gleam.toml ./remote/gleam-lambda-runner/gleam.toml
COPY remote/gleam-lambda-runner/manifest.toml ./remote/gleam-lambda-runner/manifest.toml
COPY remote/gleam-lambda-runner/src ./remote/gleam-lambda-runner/src
COPY remote/gleam-lambda-runner/child-runtimes ./remote/gleam-lambda-runner/child-runtimes
COPY remote/libs/pg-defs/generated/gleam ./remote/libs/pg-defs/generated/gleam
WORKDIR /app/remote/gleam-lambda-runner

EXPOSE 8083
CMD ["gleam", "run"]
