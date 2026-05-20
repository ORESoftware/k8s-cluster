# `remote/deployments/queue-consumer-rs`

Rust NATS queue consumer for the remote-dev shadow and queued execution paths.

The current production request path still works like this:

1. Rust REST API receives `/api/agents/threads/:threadId/tasks`.
2. Rust REST API creates or wakes the matching per-thread worker.
3. Rust REST API forwards the request directly to that worker.

This service reads `dd.remote.thread.*.tasks` through a durable JetStream pull
consumer. Shadow messages still call the REST API's internal prepare endpoint:

```text
POST /api/agents/threads/:threadId/prepare
```

Real `task.dispatch` messages are handed to a repo-scoped warm Node chat/Claude
container pool first. If that pool is unavailable and fallback is enabled, the
consumer forwards the same task to the REST API direct-dispatch endpoint so the
deterministic per-thread worker can execute it.

## Environment

| Variable | Default | Purpose |
| --- | --- | --- |
| `NATS_URL` | `nats://dd-nats.messaging.svc.cluster.local:4222` | NATS client URL. |
| `NATS_TASK_SUBJECT` | `dd.remote.thread.*.tasks` | Subject filter for shadow task messages. |
| `NATS_QUEUE_GROUP` | `dd-remote-thread-preparer` | Backward-compatible name used as the durable consumer default. |
| `NATS_TASK_STREAM` | `DD_REMOTE_TASKS` | JetStream stream created/read by the producer and consumer. |
| `NATS_TASK_CONSUMER` | `dd-remote-thread-preparer` | Durable pull consumer watched by KEDA. |
| `NATS_TASK_ACK_WAIT_SECONDS` | `120` | Redelivery window for unacked messages. |
| `NATS_TASK_MAX_ACK_PENDING` | `256` | Max in-flight unacked messages on the durable consumer. |
| `NATS_TASK_MAX_DELIVER` | `5` | Max redeliveries before JetStream stops retrying a poison message. |
| `NATS_TASK_NAK_DELAY_SECONDS` | `15` | Delay before redelivery when the REST prepare call fails. |
| `REMOTE_REST_API_URL` | `http://dd-remote-rest-api.default.svc.cluster.local:8082` | Internal REST API URL. |
| `CONTAINER_POOL_BASE_URL` | `http://dd-container-pool.default.svc.cluster.local:8102` | Internal warm worker pool URL used for real queued dispatches. |
| `QUEUE_CONSUMER_FALLBACK_REST_DISPATCH` | `true` | Fall back to direct REST dispatch when the repo-matched pool is unavailable. |
| `REMOTE_DEV_SERVER_SECRET` / `SERVER_AUTH_SECRET` | `dd-k8s-home` | Shared internal auth header for prepare calls. |
| `QUEUE_CONSUMER_RECEIPTS_DIR` | `/tmp/dd-remote-queue-consumer/tasks` | JSON task receipts used to skip duplicate NATS deliveries. |

## Scaling

`remote/argocd/dd-next-runtime/dd-remote-queue-consumer.scaledobject.yaml` uses
KEDA's NATS JetStream scaler. KEDA monitors lag for stream `DD_REMOTE_TASKS` and
consumer `dd-remote-thread-preparer`, keeps one pod running when traffic is low,
and scales out to more replicas when pending messages accumulate. All replicas
share the same durable pull consumer, so each message is delivered to one worker
and acknowledged only after the REST prepare call succeeds.

## Thread Affinity

The consumer does not assign coding-agent work to an arbitrary generic worker.
For queued execution it derives a repo-scoped pool slug from the task repo and
base branch, for example `nodejs-chat-openai-live-mutex-dev`. If that pool is
not healthy, fallback dispatch keeps the deterministic `dd-thread-<short>`
runtime available.

The consumer also keeps an in-memory taskId set and writes JSON receipts under
`QUEUE_CONSUMER_RECEIPTS_DIR`. A duplicate NATS delivery for the same taskId is
skipped after the first successful prepare call. The Node.js worker has its own
task receipt map/files too, so direct REST and future queue execution can remain
idempotent at the container boundary.
