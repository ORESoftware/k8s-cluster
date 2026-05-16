# `remote/rest-api-rs`

Rust REST API for remote-dev data.

## Purpose

- owns RDS/Postgres access for agent task/thread/event reads
- keeps DB credentials out of the public Rust webserver
- exposes a small HTTP boundary the webserver, workers, and future queue consumers can call inside
  Kubernetes
- exposes Prometheus metrics for the observability stack

The public webserver in `remote/web-home-rs` serves HTML and calls this service over HTTP at
`REMOTE_REST_API_URL`. It should not connect to RDS directly.

## Routes

| Route                                                | Purpose                                                                                                             |
| ---------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| `GET /healthz`                                       | liveness/readiness check                                                                                            |
| `GET /metrics`                                       | Prometheus metrics                                                                                                  |
| `GET /api/agents/tasks?limit=50`                     | agent threads/tasks/PR snapshot                                                                                     |
| `GET /api/agents/tasks/:taskId/events?limit=250`     | stored task event stream for the thread UI response pane                                                            |
| `POST /api/agents/tasks/:taskId/feedback`            | append an upvote/downvote feedback event for a specific response blurb                                              |
| `GET /api/agents/threads/:threadId/context?limit=20` | thread task context for worker prompt continuation; reads Postgres when configured and falls back to runtime memory |
| `GET /api/agents/threads/:threadId/runtime`          | trimmed Kubernetes Deployment/Service/Pod state for the UUID-bound worker while it is waking or sleeping            |
| `POST /api/agents/events`                            | internal worker event ingest endpoint; writes `_events`, bumps `last_event_seq`, and marks terminal tasks finished  |
| `POST /api/agents/threads/:threadId/prepare`         | internal queue-consumer endpoint; creates/scales the matching worker and waits for readiness                        |
| `POST /api/agents/threads/:threadId/sleep`           | scale the UUID-matched thread Deployment to `0` while keeping PVC state                                             |
| `POST /api/agents/threads/:threadId/archive`         | deep-sleep the UUID-matched runtime; DB archival can be layered on here later                                       |
| `POST /api/agents/threads/:threadId/hard-delete`     | delete the UUID-matched Ingress, Service, Deployment, and PVC; GitHub PRs are not deleted                           |
| `POST /api/agents/threads/:threadId/merge-upstream`  | scale the thread worker up if needed, wait for readiness, then ask it to merge `origin/dev` into its feature branch |
| `POST /api/agents/threads/:threadId/open-pr`         | scale the worker up if needed, wait for readiness, then ask it to open or reuse a draft WIP PR                      |

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

NATS is configured separately through `NATS_URL`. On successful direct dispatch, the REST API
ensures JetStream stream `DD_REMOTE_TASKS`, publishes a shadow task message to
`dd.remote.thread.<threadId>.tasks`, and also emits `dd.remote.orchestrator.wakeup`. The queue
consumer reads the durable `dd-remote-thread-preparer` pull consumer and calls `/prepare`, but
direct REST handoff remains the source of execution for now.

So yes: task dispatch currently does both. It directly calls the deterministic thread worker for
the response path, and also publishes the same taskId/threadId to NATS as a shadow/wakeup message.
Consumers prepare the same UUID-bound container and rely on taskId receipts to de-dupe repeated
deliveries.

The lambda function API is CRUD-only:

- `GET /api/lambdas/functions`
- `POST /api/lambdas/functions`
- `PATCH /api/lambdas/functions/:id`

Invocation is deliberately outside this REST service. The gateway sends
`POST /lambdas/invoke/<slug>` directly to the Gleam lambda runner.

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
