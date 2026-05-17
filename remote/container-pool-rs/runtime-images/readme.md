# Container Pool Runtime Images

These multi-stage Dockerfiles define the default warm worker images referenced by
`remote/databases/pg/seeds/container-pool-app-config.sql`.

Build from the `remote/container-pool-rs` directory so the Docker context contains
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

- `GET /healthz`
- `POST /invoke`

The worker executes the trusted `DD_POOL_HANDLER` configured in the image or app config. Dispatch
requests supply JSON payloads only; they do not choose shell commands.
