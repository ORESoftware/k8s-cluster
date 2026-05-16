# Future NATS Queue Design

This is the future design for moving remote agent task dispatch from direct HTTP
proxying to a NATS JetStream-backed queue. The current production path should
stay direct until this design is implemented and tested end to end.

## Goals

- Accept tasks even when a thread worker is asleep or cold-starting.
- Keep one logical worker runtime per chat/thread UUID.
- Ensure only one worker claims each task.
- Preserve per-thread ordering when needed.
- Keep worker state durable in Postgres plus the thread workspace PVC.
- Let the reaper/cron services react to queue state without becoming a second
  competing scheduler.

## Core Rule

Do not put all Node.js workers in one generic queue group for task execution.
NATS queue groups load-balance messages across subscribers. That is useful for
stateless work, but it is not enough for thread-affine work where a UUID must
always route to the matching runtime.

Instead:

- The REST API produces tasks onto a thread-specific subject.
- The thread worker consumes only its own thread-specific subject.
- A separate orchestrator wakes or creates the matching worker when backlog
  exists.

The literal Linux container may be replaced after sleep, restart, or node
eviction. The guarantee we want is stronger and more useful: the same logical
thread runtime resumes, with the same thread UUID, same Postgres history, same
PVC workspace, same branch, and same NATS durable cursor.

## Proposed Deployments

| Deployment | Language | Role |
| --- | --- | --- |
| `dd-remote-rest-api` | Rust | Public/internal REST API. Validates requests, writes Postgres rows, publishes NATS messages. This can be the first queue producer. |
| `dd-remote-queue-producer` | Rust, optional | Thin producer service if we want to decouple HTTP request handling from NATS publishing. Not required at first. |
| `dd-remote-thread-orchestrator` | Rust preferred, Gleam possible | Queue consumer/controller. Watches task/lifecycle subjects, acquires per-thread locks, creates or scales the matching worker Deployment, waits for readiness. |
| `dd-thread-<short>` | Node.js | One Deployment per thread. Replicas are `0` or `1`. Consumes only `dd.remote.thread.<threadId>.tasks`. Runs Claude/OpenAI/Gemini work. |
| `dd-idle-reaper` | Rust/Gleam | Scales idle thread Deployments to zero after no running tasks and no unacked queue work. It can publish lifecycle commands but should share the orchestrator lock rules. |
| `dd-chron-service` | Rust/Gleam | Publishes scheduled prompts or maintenance commands to NATS. It should not bypass the normal task producer/orchestrator path. |

Rust is the safest first choice for `dd-remote-thread-orchestrator` because the
Kubernetes and NATS client ecosystems are mature (`kube-rs`, `async-nats`).
Gleam is fine for a smaller websocket/status/event service, and could work for
the orchestrator if we keep the Kubernetes API surface tiny.

## Subjects

Use stable, thread-addressed subjects:

```text
dd.remote.thread.<threadId>.tasks
dd.remote.thread.<threadId>.control
dd.remote.thread.<threadId>.events
dd.remote.thread.<threadId>.heartbeat
dd.remote.orchestrator.wakeup
dd.remote.cron.prompts
```

UUIDs contain hyphens but no dots, so each UUID remains one NATS subject token.

## Streams

| Stream | Subjects | Retention |
| --- | --- | --- |
| `DD_REMOTE_TASKS` | `dd.remote.thread.*.tasks` | JetStream file storage, explicit ack, message dedupe by `taskId`. |
| `DD_REMOTE_CONTROL` | `dd.remote.thread.*.control`, `dd.remote.orchestrator.wakeup` | Short retention, explicit ack. |
| `DD_REMOTE_EVENTS` | `dd.remote.thread.*.events`, `dd.remote.thread.*.heartbeat` | Longer retention for replay/debugging, mirrored into Postgres. |
| `DD_REMOTE_CRON` | `dd.remote.cron.prompts` | Explicit ack, cron/maintenance initiated jobs. |

## Task Message

```json
{
  "version": 1,
  "threadId": "b82e5724-0273-4cd9-a198-ed6caac99a33",
  "taskId": "89141d07-45fe-476f-bf4d-c518b48df964",
  "taskKind": "agent.prompt",
  "provider": "claude-sdk",
  "repo": "git@github.com:ORESoftware/k8s-cluster.git",
  "baseBranch": "dev",
  "featureBranch": "dev/b82e5724-0273-4cd9-a198-ed6caac99a33/example-title",
  "prompt": "User-visible prompt text",
  "createdAt": "2026-05-15T23:00:00Z",
  "attempt": 1
}
```

Publish with a NATS message id derived from the task id:

```text
Nats-Msg-Id: remote-task:<taskId>
```

This gives JetStream publisher-side dedupe. Postgres still remains the real
idempotency guard.

## Request Flow

1. Browser calls Rust REST API.
2. REST API validates auth and payload.
3. REST API upserts `agent_remote_dev_threads`.
4. REST API inserts `agent_remote_dev_tasks` with status `queued`.
5. REST API publishes to `dd.remote.thread.<threadId>.tasks`.
6. REST API publishes a lightweight wakeup signal to `dd.remote.orchestrator.wakeup`.
7. Orchestrator acquires the per-thread lock.
8. Orchestrator creates or scales `dd-thread-<short>` to `1`.
9. Worker starts with `REMOTE_DEV_THREAD_ID=<threadId>`.
10. Worker subscribes only to `dd.remote.thread.<threadId>.tasks`.
11. Worker claims the task in Postgres with an atomic status update.
12. Worker runs the model, emits events, pushes a PR if applicable.
13. Worker marks task terminal in Postgres and acks the NATS message.

## Avoiding Duplicate Workers

The orchestrator must be safe to run with more than one replica. Use one of:

- Kubernetes `Lease` per thread, e.g. `dd-thread-lock-<short>`.
- Postgres advisory lock keyed by thread UUID.
- A small `agent_remote_dev_runtime_locks` table with `thread_id`, `owner`,
  `lease_expires_at`, and compare-and-swap updates.

Kubernetes `Lease` is the most native option if the orchestrator already has
Kubernetes permissions.

The worker Deployment itself should stay `replicas: 0` or `replicas: 1`. That is
the simplest hard guarantee that only one pod for a thread can run task code at a
time. If we later need parallel subtasks within a thread, that should be an
explicit design change with separate subtask queues.

## Avoiding Duplicate Task Claims

JetStream is at-least-once. Duplicate delivery can happen after crashes,
network partitions, slow acks, or restarts. The worker must claim tasks
idempotently:

```sql
update agent_remote_dev_tasks
set status = 'running',
    updated_at = now()
where id = $1
  and thread_id = $2
  and status in ('queued', 'retrying')
returning id;
```

If no row returns, the task was already claimed or finished. The worker should
ack the duplicate message and do nothing.

For long-running model calls, the worker should periodically extend the NATS ack
deadline with progress/in-progress acks. It should final-ack only after the
terminal state and final events are durably written.

## Wake From Zero

When no pod is running, no worker is subscribed to the thread subject. That is
fine: JetStream stores the message.

The orchestrator is the always-on consumer that notices backlog or wakeup
events. It does not execute the task. It only ensures the matching thread worker
exists and is ready. The worker then pulls its own thread messages.

This keeps ownership clean:

- REST API produces tasks.
- Orchestrator owns Kubernetes lifecycle.
- Worker owns task execution.
- Reaper owns idle scale-down.

## Reaper And Cron

The reaper can read NATS state, but it should not race the orchestrator.

Safe reaper behavior:

- Read Postgres for last activity and running tasks.
- Read JetStream consumer/stream info for pending messages per thread.
- If no running tasks and no pending thread messages for 5-15 minutes, acquire
  the same per-thread lock and scale the Deployment to `0`.
- Publish `dd.remote.thread.<threadId>.control` events for auditability.

Safe cron behavior:

- Cron creates normal task rows through REST API or a producer service.
- Cron publishes scheduled prompts to `dd.remote.cron.prompts`.
- A producer/orchestrator converts cron prompts into normal per-thread task
  messages, so cron jobs do not bypass idempotency or affinity.

## Consumer Naming

For each thread worker, use a durable consumer name derived from the thread:

```text
worker.<threadShort>
```

Filter it to:

```text
dd.remote.thread.<threadId>.tasks
```

Because the Deployment has at most one replica, only one pod should use that
durable consumer at a time. If a pod crashes, the replacement pod resumes the
same durable cursor and unacked work.

## Why Not A Generic Queue Group?

A generic queue group like `agent-workers` would let any worker receive any
message. That breaks thread affinity unless every worker then forwards the
message elsewhere, which recreates a router inside the worker fleet.

The safer shape is:

- Generic orchestration queue for lifecycle events.
- Thread-specific task queue for actual work.
- Postgres as the source of truth for task/thread state.
- Kubernetes as the source of truth for whether a thread runtime is alive.

## Current Shadow Implementation

The first shadow step is now available in code:

1. REST API continues to direct-dispatch to the worker.
2. REST API also publishes the task message to NATS with `shadow: true`.
3. `dd-remote-queue-consumer` subscribes to `dd.remote.thread.*.tasks` in queue group
   `dd-remote-thread-preparer`.
4. The consumer calls the REST API's internal `/api/agents/threads/<threadId>/prepare`
   endpoint, which creates/scales the deterministic per-thread Deployment.

This prepares the right logical thread runtime but does not execute the task
from the queue yet.

## Next Implementation Step

Keep the current direct path live while proving durability and ownership:

1. Add JetStream stream/consumer creation for `DD_REMOTE_TASKS`.
2. Record queue publish/consume metrics in Grafana.
3. Compare direct-dispatch results against queue observations.
4. Switch one test thread to queue execution.
5. Expand after idempotency, locking, and wake-from-zero are proven.
