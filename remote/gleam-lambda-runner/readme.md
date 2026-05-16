# `remote/gleam-lambda-runner`

Gleam HTTP service for running user-defined lambda functions in reusable child processes.

- `GET /healthz` returns service health.
- `GET /metrics` exposes Prometheus counters and gauges.
- `POST /invoke/:function_id` forwards one request envelope to a child process.
- `POST /destroy/:reuse_key` closes a cached child process.

The Rust REST API is responsible for CRUD/read models over Postgres. Invocation traffic goes
directly through the load balancer/gateway to this Gleam service. The BEAM runner loads the active
function definition from Postgres by immutable function UUID, then maps that UUID to a reusable
worker actor and Node child process. The Node child receives the definition over stdio, so it does
not need database credentials or `psql`.

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

## Local toolchain

If `gleam` or `erlc` are not installed on the host, use the repo machine's Nix toolchain instead of
installing global packages:

```sh
nix shell nixpkgs#gleam nixpkgs#erlang nixpkgs#rebar3 nixpkgs#nodejs_25 nixpkgs#postgresql -c sh -c 'cd remote/gleam-lambda-runner && gleam build'
```

`rebar3` is required because one of the resolved dependencies builds with Rebar. `manifest.toml` is
committed so local builds and cluster builds resolve the same package versions.
