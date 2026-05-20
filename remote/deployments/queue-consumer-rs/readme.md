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

Real `task.dispatch` messages are handed to a repo-scoped warm Node chat/Claude
container pool. There is no direct REST fallback for real queued messages, so a
queued task has one owner: the queue consumer and container pool.

## Environment

| Variable | Default | Purpose |
| --- | --- | --- |
| `NATS_URL` | `nats://dd-nats.messaging.svc.cluster.local:4222` | NATS client URL. |
| `NATS_TASK_SUBJECT` | `dd.remote.thread.*.tasks` | Subject filter for queued task messages. |
| `NATS_QUEUE_GROUP` | `dd-remote-thread-preparer` | Backward-compatible name used as the durable consumer default. |
| `NATS_TASK_STREAM` | `DD_REMOTE_TASKS` | JetStream stream created/read by the producer and consumer. |
| `NATS_TASK_CONSUMER` | `dd-remote-thread-preparer` | Durable pull consumer watched by KEDA. |
| `NATS_EVENT_SUBJECT` | `dd.remote.events` | Status-event subject bridged into telemetry and websocket fanout. The consumer also includes `threadId` in REST event ingest so the REST API can direct-fanout status over websocket if NATS event publishing is degraded. |
| `NATS_TASK_ACK_WAIT_SECONDS` | `120` | Redelivery window for unacked messages. |
| `NATS_TASK_MAX_ACK_PENDING` | `256` | Max in-flight unacked messages on the durable consumer. |
| `NATS_TASK_MAX_DELIVER` | `5` | Max redeliveries before JetStream stops retrying a poison message. |
| `NATS_TASK_NAK_DELAY_SECONDS` | `15` | Delay before redelivery when the REST prepare call fails. |
| `REMOTE_REST_API_URL` | `http://dd-remote-rest-api.default.svc.cluster.local:8082` | Internal REST API URL. |
| `CONTAINER_POOL_BASE_URL` | `http://dd-container-pool.default.svc.cluster.local:8102` | Internal warm worker pool URL used for real queued dispatches. |
| `REMOTE_DEV_SERVER_SECRET` / `SERVER_AUTH_SECRET` | `dd-k8s-home` | Shared internal auth header for prepare calls. |
| `QUEUE_CONSUMER_RECEIPTS_DIR` | `/tmp/dd-remote-queue-consumer/tasks` | JSON task receipts used to skip duplicate NATS deliveries. |

## Scaling

`remote/argocd/dd-next-runtime/dd-remote-queue-consumer.scaledobject.yaml` uses
KEDA's NATS JetStream scaler. KEDA monitors lag for stream `DD_REMOTE_TASKS` and
consumer `dd-remote-thread-preparer`, keeps one pod running when traffic is low,
and scales out to more replicas when pending messages accumulate. All replicas
share the same durable pull consumer, so each message is delivered to one worker
and acknowledged only after the queued handoff succeeds.

## Thread Affinity

The consumer does not assign coding-agent work to an arbitrary generic worker.
For queued execution it derives a repo-scoped pool slug from the task repo and
base branch, for example `nodejs-chat-openai-live-mutex-dev`, then sends the
task to that pool with `threadId` as the affinity key.

The consumer also keeps an in-memory taskId set and writes JSON receipts under
`QUEUE_CONSUMER_RECEIPTS_DIR`. A duplicate NATS delivery for the same taskId is
skipped after the first successful prepare call. The Node.js worker has its own
task receipt map/files too, so direct REST and future queue execution can remain
idempotent at the container boundary.
