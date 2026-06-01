# `remote/deployments/gleam-lambda-runner`

Gleam HTTP service for running user-defined lambda functions in reusable child processes and
optional non-root containers.

- `GET /healthz` returns service health.
- `GET /metrics` exposes Prometheus counters and gauges.
- `POST /invoke/:function_id` forwards one request envelope to a child process.
- `POST /check` compiles or syntax-checks a posted lambda definition without executing the
  function body.
- `POST /destroy/:reuse_key` closes a cached child process.

`POST /invoke/:function_id`, `POST /check`, and `POST /destroy/:reuse_key` fail closed unless
`LAMBDA_SERVER_AUTH_SECRET`, `SERVER_AUTH_SECRET`, or `REMOTE_DEV_SERVER_SECRET` is configured.
Callers must present the secret in `X-Server-Auth`, `X-Lambda-Runner-Auth`, or `X-Agent-Auth`.
`GET /healthz` and `GET /metrics` remain unauthenticated for Kubernetes probes and scraping.

The runner also starts an in-process NATS singleton when `NATS_URL` is configured. The Gleam module
`gleam_lambda_runner/nats.gleam` owns the app-facing interface and delegates the raw TCP protocol to
`lambda_nats.erl` inside the same BEAM VM. It subscribes to lambda invocation messages, invokes the
same child runner used by HTTP, and publishes invocation results back to NATS:

| Env | Default |
| --- | --- |
| `NATS_URL` | unset, disables NATS |
| `NATS_LAMBDA_INVOKE_SUBJECT` | `dd.remote.lambdas.invoke.*` |
| `NATS_LAMBDA_QUEUE_GROUP` | `dd-gleam-lambda-runner` |
| `NATS_LAMBDA_RESULT_SUBJECT` | `dd.remote.lambdas.results` |
| `NATS_LAMBDA_FUNCTIONS_SUBJECT` | `dd.remote.lambdas.functions` |
| `NATS_USERNAME` / `NATS_PASSWORD` | unset, optional NATS user/pass auth |
| `NATS_TOKEN` | unset, optional NATS token auth |

Invocation messages can either target a function through the subject suffix
(`dd.remote.lambdas.invoke.<function-id-or-slug>`) or include `functionId`, `function_id`, `slug`,
or `id` in the JSON payload. The request body passed to the lambda is the message `payload` field,
then `request`, then the full message as a fallback.

This service is function-definition-aware: it looks up an active lambda in Postgres and runs it in a
managed host child or per-function runtime container. The Rust container pool is the lower-level
generic warm-container service: it reads pool configuration from Postgres, keeps runtime containers
warm, health-checks and replaces them, then forwards HTTP/NATS work to whichever container is
leased. Use the pool for runtime-shaped work queues; use this runner when invocation must resolve a
stored lambda function definition.

The Rust REST API is responsible for CRUD/read models over Postgres. Invocation traffic goes
directly through the load balancer/gateway to this Gleam service. The BEAM runner loads the active
function definition from Postgres by immutable function UUID, then maps the function runtime to a
reusable worker actor. The managed runtimes are `nodejs`, `python3`, `ruby`, and `bash`; legacy
`javascript`, `typescript`, `python`, and `shell` values normalize to those runtime pools. Each
child receives the definition over stdio, so it does not need database credentials or `psql`.
`POST /check` uses the same runtime mapping and host/container policy as invocation: containerized
definitions are checked inside their managed runtime image, while host checks are limited by
`LAMBDA_ALLOW_HOST_RUNTIMES`.

Today `/check` is deliberately compile-only. If we add execution-style draft checks later, keep them
as a separate dry-run mode with `LAMBDA_CONTAINER_NETWORK=none` plus runtime-level `fetch`/HTTP
stubs that return deterministic `200` responses, rather than widening the build server into an
untrusted code runner.

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

Python, Ruby, and Bash do not have a reliable in-process filesystem sandbox. The API and runner
therefore require `containerized: true` for those runtimes by default. Host execution is limited to
Node.js unless `LAMBDA_ALLOW_HOST_RUNTIMES` is explicitly widened for a trusted environment. The
managed host commands can be overridden by trusted deployment/local env with
`LAMBDA_NODEJS_HOST_COMMAND`, `LAMBDA_PYTHON3_HOST_COMMAND`, `LAMBDA_RUBY_HOST_COMMAND`, and
`LAMBDA_BASH_HOST_COMMAND`; this is mainly for dev machines whose local Node permission flags lag the
cluster `nodejs-current` package. The
container path supports `LAMBDA_CONTAINER_RUNNER=nerdctl` and `LAMBDA_CONTAINER_RUNNER=ctr`.
`nerdctl` uses `--read-only --tmpfs /tmp --user 10001:10001 --cap-drop ALL --security-opt
no-new-privileges --pids-limit 64 --ulimit nofile=64:64`; `ctr` uses equivalent containerd flags
for read-only rootfs, tmpfs `/tmp`, non-root user, `LAMBDA_CONTAINER_NETWORK`-selected networking,
seccomp, memory/CPU limits, and dropped default capabilities. No host code is mounted into packaged
function images.

The manager prewarms one Node.js host worker by default via `LAMBDA_PREWARM_RUNTIMES`.
`LAMBDA_PREWARM_CONTAINER_RUNTIMES` can also warm container workers when the runtime images below
exist in the EC2 node's local containerd image store.

On EC2 Kubernetes, launching those nested containerd containers from the runner pod requires the
host `/run/containerd` directory for the socket/FIFOs, the host `/var/lib/containerd` snapshot tree,
and a privileged runner pod (or an equivalent trusted host-side helper). The EC2 manifest sets
`LAMBDA_CONTAINER_NETWORK=host` for the nested `ctr` containers because the node's Cilium CNI path
is not a stable generic CNI entrypoint from inside that trusted pod. Treat the runner pod as
node-level infrastructure: keep invocation and CRUD routes authenticated, and rely on the
per-lambda runtime flags above for the untrusted function containers.

The EC2 manifest includes a `startupProbe` on `/healthz` so package install, dependency download,
and Gleam build work at boot do not trip liveness before the service is ready.

## Runtime images

Build the reusable container pool images from the repository root:

```sh
nerdctl -n k8s.io build -f remote/deployments/gleam-lambda-runner/runtime-images/nodejs.Dockerfile -t docker.io/library/dd-lambda-nodejs-runtime:dev remote/deployments/gleam-lambda-runner
nerdctl -n k8s.io build -f remote/deployments/gleam-lambda-runner/runtime-images/python3.Dockerfile -t docker.io/library/dd-lambda-python3-runtime:dev remote/deployments/gleam-lambda-runner
nerdctl -n k8s.io build -f remote/deployments/gleam-lambda-runner/runtime-images/ruby.Dockerfile -t docker.io/library/dd-lambda-ruby-runtime:dev remote/deployments/gleam-lambda-runner
nerdctl -n k8s.io build -f remote/deployments/gleam-lambda-runner/runtime-images/bash.Dockerfile -t docker.io/library/dd-lambda-bash-runtime:dev remote/deployments/gleam-lambda-runner
```

When the REST API has `LAMBDA_IMAGE_BUILD_ENABLED=true`, saving a containerized function also writes
a per-function build context under `LAMBDA_IMAGE_BUILD_ROOT` and builds
`docker.io/library/dd-lambda-function:<slug>-<id>` into the same local `k8s.io` image store.

## Local toolchain

If `gleam` or `erlc` are not installed on the host, use the repo machine's Nix toolchain instead of
installing global packages:

```sh
nix shell nixpkgs#gleam nixpkgs#erlang nixpkgs#rebar3 nixpkgs#nodejs_25 nixpkgs#postgresql -c sh -c 'cd remote/deployments/gleam-lambda-runner && gleam build'
```

`rebar3` is required because one of the resolved dependencies builds with Rebar. `manifest.toml` is
committed so local builds and cluster builds resolve the same package versions.

Build the standalone image from the repository root so Docker can include the sibling `pg-defs`
package:

```sh
docker build -f remote/deployments/gleam-lambda-runner/Dockerfile -t dd-gleam-lambda-runner:dev .
```
