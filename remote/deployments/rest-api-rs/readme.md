# `remote/deployments/rest-api-rs`

Rust REST API for remote-dev data.

## Purpose

- owns RDS/Postgres access for agent task/thread/event reads
- keeps DB credentials out of the public Rust webserver
- exposes a small HTTP boundary the webserver, workers, and future queue consumers can call inside
  Kubernetes
- exposes Prometheus metrics for the observability stack

The public webserver in `remote/deployments/web-home-rs` serves HTML and calls this service over HTTP at
`REMOTE_REST_API_URL`. It should not connect to RDS directly.

## Routes

The served route docs are generated from source by `remote/tools/generate-api-docs.mjs`; this
README is narrative context, not the route inventory source of truth. HTML is available at
`/docs/api` and `/api/docs`; JSON metadata is available at `/api/docs.json`.

| Route                                                | Purpose                                                                                                             |
| ---------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| `GET /healthz`                                       | liveness/readiness check                                                                                            |
| `GET /metrics`                                       | Prometheus metrics                                                                                                  |
| `GET /api/agents/tasks?limit=50`                     | agent threads/tasks/PR snapshot                                                                                     |
| `GET /api/agents/git-repos?limit=100`                | known git repos registered for remote-dev threads                                                                   |
| `POST /api/agents/git-repos`                         | upsert a known git repo URL/default branch before launching a thread                                                |
| `GET /api/agents/tasks/:taskId/events?limit=250`     | stored task event stream for the thread UI response pane                                                            |
| `POST /api/agents/tasks/:taskId/feedback`            | append an upvote/downvote feedback event for a specific response blurb                                              |
| `GET /api/agents/threads/:threadId/context?limit=20` | thread task context for worker prompt continuation; reads Postgres when configured and falls back to runtime memory |
| `GET /api/agents/threads/:threadId/runtime`          | trimmed Kubernetes Deployment/Service/Pod state for the UUID-bound worker while it is waking or sleeping            |
| `POST /api/agents/events`                            | internal worker event ingest endpoint; writes `_events`, bumps `last_event_seq`, and marks terminal tasks finished  |
| `POST /api/agents/threads/:threadId/prepare`         | internal queue-consumer endpoint; creates/scales the matching worker and waits for readiness                        |
| `POST /api/agents/threads/:threadId/sleep`           | scale the UUID-matched thread Deployment to `0` while keeping PVC state                                             |
| `POST /api/agents/threads/:threadId/archive`         | deep-sleep the UUID-matched runtime; DB archival can be layered on here later                                       |
| `POST /api/agents/threads/:threadId/hard-delete`     | delete the UUID-matched Ingress, Service, Deployment, and PVC; GitHub PRs are not deleted                           |
| `POST /api/agents/threads/:threadId/merge-upstream`  | scale the thread worker up if needed, wait for readiness, then ask it to merge its configured base branch           |
| `POST /api/agents/threads/:threadId/open-pr`         | scale the worker up if needed, wait for readiness, then ask it to open or reuse a draft WIP PR                      |
| `GET /api/lambdas/functions/:idOrSlug`               | fetch one lambda definition over HTTP so non-REST deployments do not need direct RDS TCP credentials                |
| `GET /api/container-pool/images`                     | catalog of warm-pool images + latest revision/build status (backs `/container-pool/config`)                         |
| `GET /api/container-pool/images/:slug`               | per-image detail including current revision text and last build run                                                 |
| `GET /api/container-pool/images/:slug/dockerfile`    | current Dockerfile text; `?source=disk-default` returns the on-disk default, `?revisionId=` returns a saved one     |
| `PUT /api/container-pool/images/:slug/dockerfile`    | save a new Dockerfile revision (content-addressed; duplicate saves coalesce)                                        |
| `GET /api/container-pool/images/:slug/revisions`     | last N saved revisions for an image                                                                                 |
| `GET /api/container-pool/images/:slug/builds`        | last N build+test runs for an image                                                                                 |
| `POST /api/container-pool/images/:slug/build-test`   | enqueue a `nerdctl build` + smoke-run for the editor contents or a saved revision; returns the build run id         |
| `GET /api/container-pool/builds/:buildId`            | full status + logs for a specific build run                                                                         |

`/api/container-pool/*` is an operator surface. The gateway gates it with the `dd_auth` operator
cookie and forwards `X-Server-Auth`; the REST API also verifies that header by default
(`CONTAINER_POOL_IMAGE_API_AUTH_REQUIRED=true`) so direct in-cluster requests without the service
secret are rejected. Build/test history tables are owned by
`remote/libs/pg-defs/schema/schema.sql`; the route module intentionally does not create or migrate
tables at runtime. Custom smoke-test commands are disabled unless
`CONTAINER_POOL_IMAGE_CUSTOM_TEST_COMMANDS_ENABLED=true`.

The public REST API is intentionally domain/code-first:

- Code-first routes (`/api/agents/*`, `/api/lambdas/*`) keep hand-shaped product behavior,
  validation, fan-out, orchestration, aggregation, and domain joins.
- There is no generic table-shaped `/api/db/*` product surface.

Generic database inspection remains an operator-only escape hatch under `/internal/db/*`, disabled
by default. To mount it for a trusted internal environment, set
`REST_API_INTERNAL_DB_ROUTES_ENABLED=true` or `REST_API_ENABLE_INTERNAL_DB_ROUTES=true`. These routes
still require `X-Agent-Auth` or `X-Server-Auth` to match `REMOTE_DEV_SERVER_SECRET` /
`SERVER_AUTH_SECRET`, including reads, and they must not be exposed as public gateway paths.

The internal `/internal/db/join` route supports basic safe joins without making clients write SQL:

- `left`, `right`: table names; default schema is `public`
- `leftSchema`, `rightSchema`: optional explicit non-system schemas
- `leftColumn`, `rightColumn`: validated column names for an equality join
- `join`: `inner`, `left`, `right`, or `full`; default is `inner`
- `leftColumns`, `rightColumns`: optional comma-separated column allowlists
- `order`: `left.<column>` / `right.<column>`, with optional `-`, `.desc`, or `:desc`
- `limit`, `offset`: capped the same way as row listing

The route only accepts table/column identifiers discovered from `information_schema`; it does not
accept raw SQL fragments.

Do not generate SQL or migrations from this Rust code. If a code-first route needs a new table,
column, index, or constraint, update `remote/libs/pg-defs/schema/schema.sql` manually, regenerate
`remote/libs/pg-defs`, and update the Rust implementation to match.

For RDS drift checks, use `node scripts/pg/diff/rds-vs-pg-defs.mjs`. It compares live RDS catalog
state to `remote/libs/pg-defs/schema/schema.sql` and emits a report only; it does not generate
`.sql` migration files.

`node remote/tests/check-rest-api-route-parity.mjs` checks the generated docs output, the
code-first route classifications, and the internal DB route boundary. It is a checker only; it must
not become a SQL generator.

## Data sources

Preferred database variables:

1. `AGENT_TASKS_RDS_DATABASE_URL`
2. `RDS_DATABASE_URL`
3. `AGENT_TASKS_DATABASE_URL`
4. `DATABASE_URL`

Writes also require one shared admin/user owner id:

- `AGENT_TASKS_ADMIN_USER_ID` or `REMOTE_DEV_ADMIN_USER_ID`

The SQL is Postgres-compatible and expects the remote-dev tables:

- `agent_remote_dev_threads`
- `agent_remote_dev_tasks`
- `agent_remote_dev_events`
- `lambda_functions`

During migration, the service also supports Supabase REST fallback:

- `SUPABASE_URL` or `NEXT_PUBLIC_SUPABASE_URL`
- `SUPABASE_SERVICE_ROLE_KEY` or `SUPABASE_KEY`

NATS is configured separately through `NATS_URL`. Task dispatch is NATS-first by default:
when `dispatchMode` is omitted the service uses `REST_API_DEFAULT_DISPATCH_MODE` or `queued`,
persists the task, publishes a real `task.dispatch` message to
`dd.remote.thread.<threadId>.tasks`, emits `dd.remote.orchestrator.wakeup`, and returns
`202 Accepted` without waiting for a worker container. Direct dispatch remains an explicit escape
hatch: `dispatchMode: "direct"` calls the deterministic thread worker and does not publish a task
message to NATS. Plain queued dispatch relies on the NATS consumer to create or wake the
UUID-bound deterministic worker. Explicit pool modes (`queued-pool`, `nats-pool`,
`container-pool`, or `pool`) hand the task to a repo-scoped warm Node chat/Claude pool with
`threadId` affinity.
The internal prepare route remains for legacy shadow task messages that only warm the deterministic
thread worker and do not own real task execution.

Status events are not NATS-only. When the REST API accepts a queued task, publishes the NATS handoff,
or records a NATS publish failure, it also best-effort posts the same `task-event` envelope directly
to the Gleam websocket `/broadcast` endpoint and the Rust WebRTC runtime `/runtime/broadcast`
endpoint. Connected web-home clients still dedupe by `messageId`, `threadId`, and `taskId`.
The central `dd-wal-gateway` also publishes committed `agent_remote_dev_events`
rows onto the CDC stream; this service converts those WAL-derived row changes
back into the same `dd.remote.events` websocket event envelope, giving the
Gleam and Rust websocket paths a durable PG-backed catch-up feed in addition to
the direct low-latency post.

The lambda function API is CRUD-only:

- `GET /api/lambdas/functions`
- `GET /api/lambdas/functions/:idOrSlug`
- `POST /api/lambdas/functions`
- `PATCH /api/lambdas/functions/:id`

Invocation is deliberately outside this REST service. The gateway sends
`POST /lambdas/invoke/<slug>` directly to the Gleam lambda runner.

Standard provisioned RDS Postgres does not provide an HTTP SQL endpoint. Services that should avoid
direct TCP database access should call this REST API over HTTP instead; keep the RDS URL mounted
only into `dd-remote-rest-api` unless a service explicitly needs direct SQL.

Lambda saves accept managed runtimes `nodejs`, `python3`, `ruby`, and `bash`. Host execution is
limited to `nodejs` by default; `python3`, `ruby`, and `bash` saves must set `containerized: true`
unless `LAMBDA_ALLOW_HOST_RUNTIMES` is explicitly widened for a trusted environment. Setting
`containerized: true` records container packaging metadata and, when `LAMBDA_IMAGE_BUILD_ENABLED`
is true, builds a local image with `nerdctl -n k8s.io build` into the EC2 node's containerd store.
The default image tag is `docker.io/library/dd-lambda-function:<slug>-<id>`.

On Kubernetes, the local `nerdctl` build path requires a trusted pod with the containerd socket,
the host `/var/lib/containerd` snapshot tree, and privileged mount capability. Treat that as
node-level infrastructure, not as a sandbox boundary.

Container build environment:

- `LAMBDA_IMAGE_BUILD_ENABLED` defaults to `false`
- `LAMBDA_IMAGE_BUILD_ROOT` defaults to `/var/lib/dd-lambdas`
- `LAMBDA_IMAGE_REPO_ROOT` defaults to `/opt/dd-next-1`
- `LAMBDA_IMAGE_REPOSITORY` defaults to `docker.io/library/dd-lambda-function`
- `LAMBDA_IMAGE_BUILD_NERDCTL` defaults to `/usr/local/bin/nerdctl`
- `LAMBDA_IMAGE_BUILD_NAMESPACE` defaults to `k8s.io`
- `LAMBDA_ALLOW_HOST_RUNTIMES` defaults to `nodejs`

Dispatch writes the task row before the worker is fully ready. That marks the thread active during
cold start so idle sweepers do not scale a newly-created worker to zero before `/tasks` is
accepted.

The thread UIs poll `/runtime` during that same cold start. The response is trimmed to the useful
bits: desired/ready/available replicas, Service identity, Pod phase, container readiness, restart
counts, and waiting reasons such as `ContainerCreating` or image pull delays.

Workers also set `EVENT_INGEST_URL` to this REST service inside the EC2 cluster. They send
`X-Agent-Auth: $SERVER_AUTH_SECRET`; this keeps the worker free of Drizzle/direct SQL while still
making `/agents/tasks` durable instead of memory-only.

Normal task completion now marks tasks `done` after commit/push. PR creation is not automatic. A
user must click `Open draft PR`, which emits a `pr_open` event and stores `pr_state = draft`.
Operators can also click `Make commit` to wake the thread worker, commit any current workspace
changes, and push the thread branch. `Terminal` wakes the same worker and opens the gateway-proxied
container shell at `/dd-thread/<thread-short>/terminal`.

## Kubernetes

The EC2 Argo runtime deploys this as:

- Deployment: `dd-remote-rest-api`
- Service: `dd-remote-rest-api.default.svc.cluster.local:8082`
- Gateway path: `/api/agents/`

The deployment consumes optional secrets:

- `dd-agent-secrets`
- `dd-remote-rest-api-secrets`

Use `dd-remote-rest-api-secrets` for RDS-specific credentials when this moves off Neon/Supabase.

The Argo runtime also binds the `dd-remote-rest-api` ServiceAccount to the
`dd-dev/dd-control-plane` Role so these lifecycle endpoints can manage only the per-thread
Kubernetes resources in the `dd-dev` namespace.
