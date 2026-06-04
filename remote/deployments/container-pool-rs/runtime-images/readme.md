# Container Pool Runtime Images

These multi-stage Dockerfiles define the default warm worker images referenced by
`remote/databases/pg/seeds/container-pool-app-config.sql`.

Build from the `remote/deployments/container-pool-rs` directory so the Docker context contains
`runtime-images/common`:

```sh
nerdctl -n k8s.io build -f runtime-images/nodejs.Dockerfile -t docker.io/library/dd-container-pool-nodejs-runtime:dev .
nerdctl -n k8s.io build -f runtime-images/rust.Dockerfile -t docker.io/library/dd-container-pool-rust-runtime:dev .
nerdctl -n k8s.io build -f runtime-images/golang.Dockerfile -t docker.io/library/dd-container-pool-golang-runtime:dev .
nerdctl -n k8s.io build -f runtime-images/python3.Dockerfile -t docker.io/library/dd-container-pool-python3-runtime:dev .
nerdctl -n k8s.io build -f runtime-images/dart.Dockerfile -t docker.io/library/dd-container-pool-dart-runtime:dev .
nerdctl -n k8s.io build -f runtime-images/gleamlang.Dockerfile -t docker.io/library/dd-container-pool-gleamlang-runtime:dev .
nerdctl -n k8s.io build -f runtime-images/erlang.Dockerfile -t docker.io/library/dd-container-pool-erlang-runtime:dev .
```

Each image exposes a small HTTP worker on `PORT` with:

- `GET $DD_POOL_HEALTH_PATH` (default `/healthz`)
- `POST $DD_POOL_REQUEST_PATH` (default `/invoke`)

The worker executes the trusted `DD_POOL_HANDLER` configured in the image or app config. Dispatch
requests supply JSON payloads only; they do not choose shell commands.
The Rust, Go, and Gleam/Erlang smoke handlers also accept an `expr` or `expression` string with a
simple integer expression such as `3+3` and return an `answer` field, which makes cross-runtime
lambda/container-pool probes verify real runtime execution instead of metadata-only echoes.

When `NATS_URL` is injected by the manager, the common worker publishes:

- `started` and `request.*` events to `DD_POOL_NATS_EVENT_SUBJECT`
- periodic `heartbeat` events to `DD_POOL_NATS_HEARTBEAT_SUBJECT`

The default subject convention is `dd.remote.container_pool.<poolSlug>.events` and
`dd.remote.container_pool.<poolSlug>.heartbeats`.
