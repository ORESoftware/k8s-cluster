# `remote/gleam-lambda-runner`

Gleam HTTP service for running user-defined lambda functions in reusable child processes and
optional non-root containers.

- `GET /healthz` returns service health.
- `GET /metrics` exposes Prometheus counters and gauges.
- `POST /invoke/:function_id` forwards one request envelope to a child process.
- `POST /destroy/:reuse_key` closes a cached child process.

The Rust REST API is responsible for CRUD/read models over Postgres. Invocation traffic goes
directly through the load balancer/gateway to this Gleam service. The BEAM runner loads the active
function definition from Postgres by immutable function UUID, then maps the function runtime to a
reusable worker actor. The managed runtimes are `nodejs`, `python3`, `ruby`, and `bash`; legacy
`javascript`, `typescript`, `python`, and `shell` values normalize to those runtime pools. Each
child receives the definition over stdio, so it does not need database credentials or `psql`.

The runner depends on the generated Gleam schema package at
`remote/libs/pg-defs/generated/gleam` (`dd_pg_defs`). That package is generated from the shared
Postgres contract in `remote/libs/pg-defs`, so lambda-definition reads should use those exported SQL
constants instead of private table SQL.

The deployment reads its own `dd-gleam-lambda-runner-secrets` Kubernetes secret, which is
reconciled from AWS Secrets Manager by External Secrets Operator. It does not inherit the REST API
database secret. Set `LAMBDA_DATABASE_URL` there for function-definition reads, plus any NATS or
runtime-specific credentials the runner should own independently.

Node children run with the Node permission model enabled:

```sh
node --permission --allow-net child-runtimes/js-function-runner.mjs
```

The child is started through `env -i`, so it receives no database secrets, and no filesystem write,
child-process, worker, addon, or inspector permission is granted. The deployment installs Alpine
`nodejs-current` because network permissions require Node 25 or newer.

Python children run with a small builtins set and an explicit `fetch(...)` helper. Ruby and Bash do
not have a reliable in-process filesystem sandbox, so use `containerized: true` for untrusted Ruby
or Bash functions. The container path uses `nerdctl run --read-only --tmpfs /tmp --user
10001:10001 --cap-drop ALL --security-opt no-new-privileges`, with network left enabled and no
host code mounted into packaged function images.

The manager prewarms one host worker per runtime by default via `LAMBDA_PREWARM_RUNTIMES`.
`LAMBDA_PREWARM_CONTAINER_RUNTIMES` can also warm container workers when the runtime images below
exist in the EC2 node's local containerd image store.

## Runtime images

Build the reusable container pool images from the repository root:

```sh
nerdctl -n k8s.io build -f remote/gleam-lambda-runner/runtime-images/nodejs.Dockerfile -t docker.io/library/dd-lambda-nodejs-runtime:dev remote/gleam-lambda-runner
nerdctl -n k8s.io build -f remote/gleam-lambda-runner/runtime-images/python3.Dockerfile -t docker.io/library/dd-lambda-python3-runtime:dev remote/gleam-lambda-runner
nerdctl -n k8s.io build -f remote/gleam-lambda-runner/runtime-images/ruby.Dockerfile -t docker.io/library/dd-lambda-ruby-runtime:dev remote/gleam-lambda-runner
nerdctl -n k8s.io build -f remote/gleam-lambda-runner/runtime-images/bash.Dockerfile -t docker.io/library/dd-lambda-bash-runtime:dev remote/gleam-lambda-runner
```

When the REST API has `LAMBDA_IMAGE_BUILD_ENABLED=true`, saving a containerized function also writes
a per-function build context under `LAMBDA_IMAGE_BUILD_ROOT` and builds
`docker.io/library/dd-lambda-function:<slug>-<id>` into the same local `k8s.io` image store.

## Local toolchain

If `gleam` or `erlc` are not installed on the host, use the repo machine's Nix toolchain instead of
installing global packages:

```sh
nix shell nixpkgs#gleam nixpkgs#erlang nixpkgs#rebar3 nixpkgs#nodejs_25 nixpkgs#postgresql -c sh -c 'cd remote/gleam-lambda-runner && gleam build'
```

`rebar3` is required because one of the resolved dependencies builds with Rebar. `manifest.toml` is
committed so local builds and cluster builds resolve the same package versions.

Build the standalone image from the repository root so Docker can include the sibling `pg-defs`
package:

```sh
docker build -f remote/gleam-lambda-runner/Dockerfile -t dd-gleam-lambda-runner:dev .
```
