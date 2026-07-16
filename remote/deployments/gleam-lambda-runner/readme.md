# `remote/deployments/gleam-lambda-runner`

Gleam HTTP service for running user-defined lambda functions in reusable child processes and
optional non-root containers.

- `GET /healthz` returns service health.
- `GET /metrics` exposes Prometheus counters and gauges.
- `POST /invoke/:function_id` forwards one request envelope to a child process.
- `POST /check` compiles or syntax-checks a posted lambda definition without executing the
  function body.
- `POST /destroy/:reuse_key` closes a cached child process.
- `POST /workflows/start` starts a durable workflow run.
- `GET /workflows/runs` lists recent runs (optional `?definition=<id|slug>&limit=N`).
- `GET /workflows/runs/:run_id` returns one run plus its step-run history.
- `POST /workflows/runs/:run_id/signal` delivers an external signal to a run.
- `POST /workflows/runs/:run_id/cancel` cancels a non-terminal run.

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
 reusable worker actor. The managed host runtime is `nodejs`; `python3`, `ruby`, `bash`, `golang`,
 `dart`, `erlang`, `elixir`, and `java` are supported as managed container runtimes. Legacy
 `javascript`, `typescript`, `python`, `shell`, `go`, `erl`, `ex`, and `jvm` values normalize to
 those runtime pools. Each child receives the definition over stdio, so it does not need database
 credentials or `psql`.
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

Python, Ruby, Bash, Go, Dart, Erlang, Elixir, and Java do not have a reliable in-process filesystem
sandbox in this service. The API and runner therefore require `containerized: true` for those
runtimes by default. Host execution is limited to Node.js unless `LAMBDA_ALLOW_HOST_RUNTIMES` is
explicitly widened for a trusted environment. The
managed host commands can be overridden by trusted deployment/local env with
`LAMBDA_NODEJS_HOST_COMMAND`, `LAMBDA_PYTHON3_HOST_COMMAND`, `LAMBDA_RUBY_HOST_COMMAND`, and
`LAMBDA_BASH_HOST_COMMAND`; this is mainly for dev machines whose local Node permission flags lag the
cluster `nodejs-current` package. The
container path supports `LAMBDA_CONTAINER_RUNNER=nerdctl` (default), `LAMBDA_CONTAINER_RUNNER=ctr`,
`LAMBDA_CONTAINER_RUNNER=docker`, and `LAMBDA_CONTAINER_RUNNER=podman`. The Docker-CLI compatible
runners (`nerdctl`, `docker`, `podman`) share the same hardening flags: `--read-only --tmpfs /tmp
--user 10001:10001 --cap-drop ALL --security-opt no-new-privileges --pids-limit 64 --ulimit
nofile=64:64`. `nerdctl` additionally scopes containers to the `LAMBDA_CONTAINER_NAMESPACE`
containerd namespace via `-n`, which `docker`/`podman` do not have. `ctr` uses equivalent containerd
flags for read-only rootfs, tmpfs `/tmp`, non-root user, `LAMBDA_CONTAINER_NETWORK`-selected
networking, seccomp, memory/CPU limits, and dropped default capabilities. The runner binary path is
overridable per backend via `LAMBDA_CONTAINER_NERDCTL`, `LAMBDA_CONTAINER_CTR`,
`LAMBDA_CONTAINER_DOCKER`, and `LAMBDA_CONTAINER_PODMAN`. No host code is mounted into packaged
function images.

The manager prewarms one Node.js host worker by default via `LAMBDA_PREWARM_RUNTIMES`.
`LAMBDA_PREWARM_CONTAINER_RUNTIMES` can also warm container workers when the runtime images below
exist in the EC2 node's local containerd image store.

### Playwright and Puppeteer as Node.js lambda capabilities

Containerized Node.js lambdas can opt into first-class Chromium automation by
setting `metaData.browserAutomation=true` (or the equivalent top-level field).
The managed Node image includes `playwright-core`, `puppeteer-core`, Chromium,
fonts, and CA certificates. The runner exposes fixed launch helpers and closes
every browser it launched when the invocation finishes, including error paths:

```js
const browser = await context.browser.launchPlaywright();
const page = await browser.newPage();
await page.goto(request.url, { waitUntil: "domcontentloaded" });
return { title: await page.title() };
```

Use `context.browser.launchPuppeteer()` for the Puppeteer API. Browser helpers
are intentionally unavailable to host-mode functions: browser lambdas must use
`containerized: true`, keeping child processes, filesystem access, and Chromium
inside the non-root, read-only runtime container. Check responses report
`browserAutomation` and `browserEngines`, so clients can verify the capability
before invocation. Browser-enabled definitions receive separate bounded defaults
(`1g` memory, `1` CPU, `256m` tmpfs, 256 PIDs); tune them with
`LAMBDA_BROWSER_CONTAINER_MEMORY`, `LAMBDA_BROWSER_CONTAINER_MEMORY_BYTES`,
`LAMBDA_BROWSER_CONTAINER_CPUS`, `LAMBDA_BROWSER_CONTAINER_TMPFS_SIZE`, and
`LAMBDA_BROWSER_CONTAINER_PIDS_LIMIT`.

The digest-pinned browser image uses Node 22's permission model to restrict
filesystem reads/writes and allow the Chromium child process. Node 22 does not
offer a network permission flag; destination policy therefore belongs at the
container/pod network boundary and in the scraping service's SSRF controls.
The generated NATS subject module is copied into the same repository-relative
path during the image build, so container-pool routing fails at build time if
the shared messaging contract disappears.

Browser scraping is a safe, ethical, and legitimate lambda use when it targets
public or explicitly authorized data, respects site terms and `robots.txt`,
rate-limits requests, and minimizes retained personal data. It is not permission
to evade authentication, paywalls, CAPTCHAs, blocks, or opt-outs. Keep egress
controls in place and use the dedicated `dd-web-scraper` service when callers need
its stronger per-request SSRF, redirect, proxy, and subresource policy.

## Container-pool dispatch (warm containers over NATS)

Instead of spawning a child container locally per invoke, the runner can hand an invocation to
`dd-container-pool`, which keeps warm containers reconciled and leases an idle one per request. This
trades the local cold-start for a warm-start at the cost of a NATS round trip.

The runner sends a NATS request/reply to the pool's owned subject
`dd.remote.container_pool.<language>.requests` (built from the generated
`ContainerPoolLanguageRequests` wildcard in `remote/libs/nats`, so a schema rename surfaces at build
time). The request body is the standard pool `DispatchRequest`
(`{requestId, poolSlug?, source, payload}`) where `payload` is the lambda invocation envelope; the
pool replies with a `DispatchResponse` and the runner returns its `body` as the invocation output.

Routing is opt-in per function and reads the same definition JSON used elsewhere (fields are commonly
carried in `meta_data_json`):

- `poolBacked` (bool): route this function through the pool. Global default is
  `LAMBDA_POOL_DISPATCH_DEFAULT` (default `false`).
- `poolLanguage` (string, optional): pool language token; defaults to the function's canonical
  runtime (e.g. `nodejs`). Must match `^[A-Za-z0-9_-]{1,64}$`.
- `poolSlug` (string, optional): pins a specific pool; included in the request so the pool selects by
  slug rather than inferring from the subject. Must match `^[A-Za-z0-9._:-]{1,119}$`.
- `poolSubject` (string, optional): overrides the request subject entirely; env override is
  `LAMBDA_POOL_SUBJECT`.

When a pool dispatch fails (NATS unconfigured, timeout, pool error), the runner falls back to local
execution by default so a pool outage degrades latency, not availability. Set
`LAMBDA_POOL_FALLBACK_LOCAL=false` to fail closed instead. Dispatch volume and failures are exposed
as `dd_lambda_runner_pool_dispatch_total` and `dd_lambda_runner_pool_dispatch_failures_total`.

The warm-container path requires `NATS_URL` to be configured and a pool whose worker image
understands the lambda invocation envelope (it loads the active function definition from Postgres by
`functionId`/`slug`, the same contract the local child runtimes use).

On EC2 Kubernetes, launching those nested containerd containers from the runner pod requires the
host `/run/containerd` directory for the socket/FIFOs, the host `/var/lib/containerd` snapshot tree,
and a privileged runner pod (or an equivalent trusted host-side helper). The EC2 manifest sets
`LAMBDA_CONTAINER_NETWORK=host` for the nested `ctr` containers because the node's Cilium CNI path
is not a stable generic CNI entrypoint from inside that trusted pod. Treat the runner pod as
node-level infrastructure: keep invocation and CRUD routes authenticated, and rely on the
per-lambda runtime flags above for the untrusted function containers.

The EC2 manifest includes a `startupProbe` on `/healthz` so package installation does not trip
liveness before the service is ready. Compilation runs in a dedicated init container with an
8 GiB build-only limit and writes BEAM artifacts to a pod-local `emptyDir`; the long-running
process has a 2 GiB limit and executes the compiled entry module directly. The source hostPath is
read-only, and the init copy excludes macOS `._*` metadata so host artifacts cannot break Linux
builds or acquire ownership of build outputs.

## Runtime images

Build the reusable container pool images from the repository root:

```sh
nerdctl -n k8s.io build -f remote/deployments/gleam-lambda-runner/runtime-images/nodejs.Dockerfile -t docker.io/library/dd-lambda-nodejs-runtime:dev remote
nerdctl -n k8s.io build -f remote/deployments/gleam-lambda-runner/runtime-images/python3.Dockerfile -t docker.io/library/dd-lambda-python3-runtime:dev remote/deployments/gleam-lambda-runner
nerdctl -n k8s.io build -f remote/deployments/gleam-lambda-runner/runtime-images/ruby.Dockerfile -t docker.io/library/dd-lambda-ruby-runtime:dev remote/deployments/gleam-lambda-runner
nerdctl -n k8s.io build -f remote/deployments/gleam-lambda-runner/runtime-images/bash.Dockerfile -t docker.io/library/dd-lambda-bash-runtime:dev remote/deployments/gleam-lambda-runner
nerdctl -n k8s.io build -f remote/deployments/gleam-lambda-runner/runtime-images/golang.Dockerfile -t docker.io/library/dd-lambda-golang-runtime:dev remote/deployments/gleam-lambda-runner
nerdctl -n k8s.io build -f remote/deployments/gleam-lambda-runner/runtime-images/dart.Dockerfile -t docker.io/library/dd-lambda-dart-runtime:dev remote/deployments/gleam-lambda-runner
nerdctl -n k8s.io build -f remote/deployments/gleam-lambda-runner/runtime-images/erlang.Dockerfile -t docker.io/library/dd-lambda-erlang-runtime:dev remote/deployments/gleam-lambda-runner
nerdctl -n k8s.io build -f remote/deployments/gleam-lambda-runner/runtime-images/elixir.Dockerfile -t docker.io/library/dd-lambda-elixir-runtime:dev remote/deployments/gleam-lambda-runner
nerdctl -n k8s.io build -f remote/deployments/gleam-lambda-runner/runtime-images/java.Dockerfile -t docker.io/library/dd-lambda-java-runtime:dev remote/deployments/gleam-lambda-runner
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

## Workflow execution engine

This service also hosts a lightweight, Temporal-style workflow engine for long-running, reliable
jobs. A **workflow definition** (`workflow_definitions`) is a declarative, ordered list of steps;
each step is one of:

- `activity` — invoke a stored `lambda_functions` activity by `functionId`/`functionSlug`. The
  request the activity receives is `{ runId, step, input, context, runInput }`, where `context`
  accumulates the output of every prior step keyed by step name. Activities support a per-step
  `retry` policy (`maxAttempts`, `backoffMs`, `backoffFactor`, `maxBackoffMs`) and `timeoutMs`.
- `sleep` — a durable timer (`durationMs`) that survives restarts.
- `waitSignal` — block until an external signal named `signalName` arrives (optional
  `waitTimeoutMs`); the signal payload is merged into `context` under the step name.

A **workflow run** (`workflow_runs`) is one durable execution. The engine is a **durable
step-state machine**, not event-sourced replay: run state is committed to Postgres after every
step, so a process crash or node restart resumes from the last committed step. Activities are
therefore **at-least-once** — write them idempotently, the same contract Temporal places on
activities.

A singleton scheduler (`workflow_engine`) polls Postgres for due runs and claims them with an
atomic lease (`lease_until`) under `FOR UPDATE SKIP LOCKED`, which is safe across replicas and
does not flip a run's semantic status — so the worker always knows whether it is starting,
retrying, waking from a timer, or resuming a wait. Each due run is advanced exactly one step by a
short-lived worker process; orphaned (leased-but-dead) runs are reclaimed once their lease lapses,
which is the engine's automatic crash recovery. `workflow_step_runs` is the append-only per-attempt
history surfaced by `GET /workflows/runs/:run_id`.

The engine is enabled automatically when `LAMBDA_DATABASE_URL` is set (set
`WORKFLOW_ENGINE_ENABLED=0` to disable). All workflow HTTP routes require the same `X-Server-Auth`
secret as `/invoke`. Run lifecycle events are best-effort published to NATS
`dd.remote.workflows.events`; runs can also be started and signalled over NATS
(`dd.remote.workflows.start` request/reply and `dd.remote.workflows.signal.<run_id>`). Engine
counters are exported on `/metrics` under the `workflow_*` prefix.

| Env | Default |
| --- | --- |
| `WORKFLOW_ENGINE_ENABLED` | `1` when `LAMBDA_DATABASE_URL` is set |
| `WORKFLOW_POLL_MS` | `1000` |
| `WORKFLOW_MAX_INFLIGHT` | `16` |
| `WORKFLOW_LEASE_MS` | `60000` (crash-recovery delay for orphaned runs) |
| `WORKFLOW_CLAIM_BATCH` | `25` |
| `NATS_WORKFLOW_START_SUBJECT` | `dd.remote.workflows.start` |
| `NATS_WORKFLOW_SIGNAL_SUBJECT` | `dd.remote.workflows.signal.*` |
| `NATS_WORKFLOW_EVENT_SUBJECT` | `dd.remote.workflows.events` |

Schema lives in `remote/libs/pg-defs/schema/schema.sql` (`workflow_definitions`, `workflow_runs`,
`workflow_step_runs`); definition CRUD is the Rust REST API's responsibility, while run
orchestration is owned here. Like the lambda tables, the runner reads these through the generated
`dd_pg_defs` SQL constants rather than private table SQL.
