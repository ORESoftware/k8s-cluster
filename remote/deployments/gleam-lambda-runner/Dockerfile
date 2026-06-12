FROM ghcr.io/gleam-lang/gleam:v1.16.0-erlang-alpine
# The OpenTelemetry exporter transitively pulls in ts_chatterbox, which sets
# `warnings_as_errors` and uses a now-deprecated `catch ...` form that Erlang/OTP
# 29 promotes to a hard error. Suppressing that one deprecation warning keeps the
# dependency compiling; it is a no-op on older OTP releases.
ENV ERL_COMPILER_OPTIONS="[nowarn_deprecated_catch]"
RUN apk add --no-cache \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/main \
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
COPY remote/libs/otel-client-gleam ./remote/libs/otel-client-gleam
WORKDIR /app/remote/deployments/gleam-lambda-runner

EXPOSE 8083
CMD ["gleam", "run"]
