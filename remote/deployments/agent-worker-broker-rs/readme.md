# `remote/deployments/agent-worker-broker-rs`

Rust broker for thread-bound Node.js worker dispatch.

This service is intentionally named a broker, not a proxy. It owns the long-run dispatch boundary
between browser/API calls and per-thread worker Deployments:

- accepts authenticated task dispatch requests from the gateway
- publishes the task to NATS JetStream on `dd.remote.thread.<threadId>.tasks`
- emits `dd.remote.orchestrator.wakeup`
- checks whether the deterministic `dd-thread-<short>` worker is already healthy
- direct-posts to that worker only when it is awake
- scales the worker Deployment to `1` when it is not awake

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
