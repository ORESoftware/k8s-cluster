# `remote/deployments/agent-worker-broker-rs`

Rust broker for thread-bound Node.js worker dispatch.

This service is intentionally named a broker, not a proxy. It owns the long-run dispatch boundary
between browser/API calls and per-thread worker Deployments:

- accepts authenticated task dispatch requests from the gateway
- checks whether the deterministic `dd-thread-<short>` worker is already healthy
- direct-posts to that worker only when it is awake, without also publishing a task to NATS
- otherwise publishes the task to NATS JetStream on `dd.remote.thread.<threadId>.tasks`
- emits `dd.remote.orchestrator.wakeup` only for the queued path

Initial public route:

```text
POST /api/agent-worker/threads/:threadId/tasks
```

Request body:

```json
{
  "taskId": "task uuid",
  "threadId": "thread uuid",
  "repo": "git@github.com:org/repo.git",
  "baseBranch": "dev",
  "prompt": "user prompt",
  "provider": "claude-sdk",
  "threadTitle": "optional title"
}
```

`repo` is required. The broker forwards it to NATS and to any direct worker dispatch, and it does
not default to any repository.

The current `dd-remote-rest-api` dispatch path is still left in place. This crate is the additive
target for moving worker lifecycle and dispatch out of the REST data API.

## Hardening

- **NATS subject-injection guard** — `threadId`/`taskId` are validated against a strict allowlist
  (non-empty, ≤200 bytes, only ASCII alphanumerics plus `-` and `_`). `threadId` is interpolated raw
  into the task subject `dd.remote.thread.{threadId}.tasks`, so `.` (the token separator) and the
  wildcards `*`/`>` are rejected — a crafted id can't publish across threads or to a wildcard.
- **Constant-time auth** — the `X-Server-Auth`/`X-Agent-Auth` secret is compared in constant time.
- **Single shared NATS connection** — the broker connects once at startup (stable client name, ping,
  connect timeout, initial-connect retry, optional `NATS_CREDENTIALS_FILE`/`NATS_TOKEN`/`NATS_NKEY`
  and `NATS_REQUIRE_TLS`) and reuses it, instead of opening a fresh unauthenticated TCP connection on
  every dispatch.
- **Request limits** — a 4 MiB body limit and a non-empty, ≤1 MiB `prompt` check bound dispatch input
  below the 8 MiB JetStream message ceiling.
- **Bounded publish** — the NATS publish path is wrapped in a timeout (`NATS_PUBLISH_TIMEOUT_MS`,
  default 10s) so a half-dead broker returns `504` instead of hanging the dispatch handler.
- **Ensure-once stream** — the WorkQueue stream is ensured once at startup (not per request); a
  publish failure or timeout clears the flag so the next request re-ensures (covers runtime deletion).
- A startup warning is logged if no auth secret is configured (dispatch fails closed regardless).

> **ORM policy:** prefer **SeaORM** over sqlx for new database code (MASH stack: maud, axum, SeaORM, supabase, htmx).
