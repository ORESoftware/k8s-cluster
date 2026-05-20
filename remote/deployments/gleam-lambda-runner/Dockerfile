FROM ghcr.io/gleam-lang/gleam:v1.16.0-erlang-alpine
RUN apk add --no-cache \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/community \
  nodejs-current \
  python3 \
  ruby \
  bash \
  postgresql-client
WORKDIR /app
COPY remote/deployments/gleam-lambda-runner/gleam.toml ./remote/deployments/gleam-lambda-runner/gleam.toml
COPY remote/deployments/gleam-lambda-runner/manifest.toml ./remote/deployments/gleam-lambda-runner/manifest.toml
COPY remote/deployments/gleam-lambda-runner/src ./remote/deployments/gleam-lambda-runner/src
COPY remote/deployments/gleam-lambda-runner/child-runtimes ./remote/deployments/gleam-lambda-runner/child-runtimes
COPY remote/deployments/gleam-lambda-runner/runtime-images ./remote/deployments/gleam-lambda-runner/runtime-images
COPY remote/libs/pg-defs/generated/gleam ./remote/libs/pg-defs/generated/gleam
WORKDIR /app/remote/deployments/gleam-lambda-runner

EXPOSE 8083
CMD ["gleam", "run"]
