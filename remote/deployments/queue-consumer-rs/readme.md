# `remote/deployments/queue-consumer-rs`

Rust NATS queue consumer for the remote-dev queued execution path.

Direct dispatch still works like this:

1. Rust REST API receives `/api/agents/threads/:threadId/tasks`.
2. Rust REST API creates or wakes the matching per-thread worker.
3. Rust REST API forwards the request directly to that worker.
4. No task message is published to NATS for that accepted direct handoff.

This service reads `dd.remote.thread.*.tasks` through a durable JetStream pull
consumer. Legacy shadow messages still call the REST API's internal prepare endpoint:

```text
POST /api/agents/threads/:threadId/prepare
```

Real `task.dispatch` messages are routed by the `dispatchMode` fields in the NATS
payload. Plain `queued`/`nats`/`async` messages are handed to the UUID-bound
deterministic worker path. Only explicit `queued-pool`/`nats-pool`/
`container-pool` aliases are handed to a repo-scoped warm Node chat/Claude
container pool first. If that repo pool is missing, unhealthy, or rejects an
explicit pool request, the consumer can fall back to the deterministic worker
path. The fallback is enabled by default through `QUEUE_CONSUMER_FALLBACK_REST_DISPATCH`.

## Environment

| Variable | Default | Purpose |
| --- | --- | --- |
| `NATS_URL` | `nats://dd-nats.messaging.svc.cluster.local:4222` | NATS client URL. |
| `NATS_TASK_SUBJECT` | `dd.remote.thread.*.tasks` | Subject filter for queued task messages. |
| `NATS_QUEUE_GROUP` | `dd-remote-thread-preparer` | Backward-compatible name used as the durable consumer default. |
| `NATS_TASK_STREAM` | `DD_REMOTE_TASKS` | JetStream stream created/read by the producer and consumer. |
| `NATS_TASK_CONSUMER` | `dd-remote-thread-preparer` | Durable pull consumer watched by KEDA. |
| `NATS_EVENT_SUBJECT` | `dd.remote.events` | Status-event subject bridged into telemetry and websocket fanout. The consumer also includes `threadId` in REST event ingest so the REST API can direct-fanout status over websocket if NATS event publishing is degraded. |
| `NATS_CRITICAL_EVENT_SUBJECT` | `dd.remote.events.critical` | Redacted critical operational events for invalid task payloads, ack/NAK failures, dispatch failures, and receipt/idempotency failures. Each locally emitted critical event also writes a `dd.log.v1` JSONL line to stderr. |
| `NATS_CRITICAL_EVENT_STREAM` | `DD_REMOTE_CRITICAL_EVENTS` | JetStream stream for durable critical runtime events. |
| `NATS_CRITICAL_EVENT_CONSUMER` | `dd-runtime-critical-events` | Durable pull consumer used by the critical-event logger loop. |
| `QUEUE_CONSUMER_CRITICAL_EVENT_LOGGER` | `true` | When true, the deployment also consumes `dd.remote.events.critical`, logs compact `dd.log.v1` stderr records, and acknowledges those critical events. |
| `NATS_CRITICAL_EVENT_ACK_WAIT_SECONDS` | `60` | Redelivery window for unacked critical event messages. |
| `NATS_CRITICAL_EVENT_MAX_ACK_PENDING` | `512` | Max in-flight unacked critical event messages. |
| `NATS_CRITICAL_EVENT_MAX_DELIVER` | `5` | Max redeliveries before JetStream stops retrying a poison critical event. |
| `NATS_TASK_ACK_WAIT_SECONDS` | `120` | Redelivery window for unacked messages. |
| `NATS_TASK_MAX_ACK_PENDING` | `256` | Max in-flight unacked messages on the durable consumer. |
| `NATS_TASK_MAX_DELIVER` | `5` | Max redeliveries before JetStream stops retrying a poison message. |
| `NATS_TASK_NAK_DELAY_SECONDS` | `15` | Delay before redelivery when the REST prepare call fails. |
| `REMOTE_REST_API_URL` | `http://dd-remote-rest-api.default.svc.cluster.local:8082` | Internal REST API URL. |
| `CONTAINER_POOL_BASE_URL` | `http://dd-container-pool.default.svc.cluster.local:8102` | Internal warm worker pool URL used for real queued dispatches. |
| `QUEUE_CONSUMER_FALLBACK_REST_DISPATCH` | `true` | When true, failed pool handoff falls back to `/prepare` plus direct REST dispatch. |
| `REMOTE_DEV_SERVER_SECRET` / `SERVER_AUTH_SECRET` | `dd-k8s-home` | Shared internal auth header for prepare calls. |
| `QUEUE_CONSUMER_REQUIRE_NONDEFAULT_SECRET` | `false` | When true, refuse to start if the auth secret is still the built-in default (fail closed instead of warning). |
| `QUEUE_CONSUMER_RECEIPTS_DIR` | `/tmp/dd-remote-queue-consumer/tasks` | JSON task receipts used to skip duplicate NATS deliveries. |
| `NATS_REQUIRE_TLS` | `false` | Require TLS to the NATS broker. |
| `NATS_CREDENTIALS_FILE` / `NATS_TOKEN` / `NATS_NKEY` | _(unset)_ | Optional NATS auth (precedence in that order). |

## Hardening

- **Identifier validation** — `threadId`/`taskId` are validated on ingest against a strict allowlist
  (non-empty, ≤200 bytes, no `..`, and only ASCII alphanumerics plus `-`, `_`, `.`). They flow raw into
  REST paths (`/api/agents/threads/{threadId}/prepare`) and the receipt filename, so the allowlist blocks
  not just `/`, `\`, and control bytes but also URL-significant characters (`?`, `#`, `%`, spaces) that
  could otherwise steer the request's query/fragment. Invalid messages are logged as a critical event and
  acked (dropped) rather than retried.
- **Fail-closed auth secret** — the built-in default secret is compiled into the image, so
  `QUEUE_CONSUMER_REQUIRE_NONDEFAULT_SECRET=true` makes the consumer refuse to start when no real secret
  is configured, instead of silently authenticating with a known-public credential.
- **Collision-resistant receipts** — the receipt filename combines the sanitized id with a hash of the
  raw id, so two distinct ids can never alias the same receipt file (which would silently drop a task).
- **NATS connection** — sets a stable client name, ping interval, and connect timeout, retries the
  initial connect, and supports optional auth/TLS via the env above (previously a bare connect).
- **Graceful shutdown** — SIGTERM/SIGINT stops the loop between messages so an in-flight handoff
  finishes before exit, avoiding a redelivery storm on rolling restarts.
- **Bounded duplicate cache** — the in-memory taskId set is capped (the on-disk receipts stay the
  durable check), so a long-lived pod can't grow it without bound.
- A `WARN` is logged if the internal auth secret falls back to the built-in default.

## Scaling

`remote/argocd/dd-next-runtime/dd-remote-queue-consumer.scaledobject.yaml` uses
KEDA's NATS JetStream scaler. KEDA monitors lag for stream `DD_REMOTE_TASKS` and
consumer `dd-remote-thread-preparer`, keeps one pod running when traffic is low,
and scales out to more replicas when pending messages accumulate. All replicas
share the same durable pull consumer, so each message is delivered to one worker
and acknowledged only after the queued handoff succeeds.

## Thread Affinity

The consumer does not assign coding-agent work to an arbitrary generic worker.
For explicit pool execution it derives a repo-scoped pool slug from the task repo
and base branch, for example `nodejs-chat-claude-live-mutex-dev`, then sends the
task to that pool with `threadId` as the affinity key and `freshAffinity: true`.
That lets follow-up tasks reuse the same affinity-bound container while preventing
a brand-new thread from being assigned to an unbound worker that has already
handled another request.

The consumer also keeps an in-memory taskId set and writes JSON receipts under
`QUEUE_CONSUMER_RECEIPTS_DIR`. A duplicate NATS delivery for the same taskId is
skipped after the first successful prepare call. The Node.js worker has its own
task receipt map/files too, so direct REST and future queue execution can remain
idempotent at the container boundary.
