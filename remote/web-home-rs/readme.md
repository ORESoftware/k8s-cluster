# `remote/web-home-rs`

Rust public web layer for remote-dev.

## Purpose

- serves `GET /` and `GET /home` as the operator-facing homepage
- serves `GET /agents/tasks` as the cluster-hosted remote-dev diagnostics table
- serves `GET /agents/threads` as the thread-first chat/task UI with stored response events
- keeps HTML/public pages out of the Node.js worker runtime
- keeps database credentials out of the public HTML server
- exposes `GET /healthz` for web liveness
- exposes `GET /metrics` for Prometheus/OpenTelemetry Collector scraping
- links to the Rust PIN auth service at `/auth?return=/home`

Task dispatch / stream / cancel routing should use the per-thread Kubernetes Ingress path
(`/dd-thread/<short>/...`). Node.js is not the UUID router; each Node.js worker container is pinned
to exactly one thread and should only accept tasks for that thread.

## Agent tasks data

`/agents/tasks` and `/agents/threads` are SSR shells rendered by the Rust web layer with Maud. Each
page has same-origin CSS, JavaScript, and HTML-fragment bundle endpoints under `/assets/web-home/`.
Their browser JavaScript calls the public gateway routes `GET /api/agents/tasks?limit=...` and
`GET /api/agents/tasks/:taskId/events?limit=...` directly.

The REST API owns RDS/Postgres, Supabase fallback, and later NATS/Postgres event write paths. This
keeps page rendering separate from data access without making the webserver a proxy.

## Auth Link

Protected ops paths redirect to `/auth?return=<path>` when the request looks like a document
navigation. `dd-remote-auth` accepts the temporary operator PIN and sets the `dd_auth` cookie used
by the gateway.

## Thread chat

The `/agents/threads` page is the operator chat surface: sidebar threads, Thread Control for the
selected worker, previous tasks, stored response stream, and upvote/downvote feedback events. The
`/agents/tasks` page remains useful as the compact snapshot/diagnostics table. When dispatch has to
cold-start a UUID-bound worker, `/agents/threads` shows visible waking/still-waiting status events
instead of leaving the response pane stuck on a generic dispatch message. While the dispatch
request is still pending, the page also polls `GET /api/agents/threads/:threadId/runtime` so the
operator can see the Kubernetes Deployment, Pod phase, container readiness, and waiting reason.
Thread Control expands while creating a new worker, while selecting Previous tasks or Response
stream gives those lower panels the larger share. Existing-thread-only controls stay hidden until
the selected UUID is backed by a stored thread.

Both UI pages target the per-thread Ingress shape:

```text
/dd-thread/<thread-short>/tasks
/dd-thread/<thread-short>/stream/<taskId>
/dd-thread/<thread-short>/ws?threadId=<uuid>&taskId=<uuid>
```

`thread-short` is the same 12-character lowercase hex prefix used by
`src/lib/server/remote-dev/container-registry.ts`. When the ingress controller and per-thread
Ingress exist, Kubernetes selects the matching worker Service. The selected Node.js container then
runs the task and streams task events.

The page opens two WebSocket paths while a task is active:

- `wss://<host>/gleam/ws?threadId=<uuid>&taskId=<uuid>` for cluster-wide NATS/Gleam fanout.
- `wss://<host>/dd-thread/<thread-short>/ws?threadId=<uuid>&taskId=<uuid>` for direct worker
  replay/live events from the pinned Node.js container.

Browser-side event dedupe keeps the same task event from rendering twice when SSE, Gleam, and the
direct worker socket all report it.

The same page now exposes per-thread controls:

- `Pause/Sleep` calls the Rust REST API to scale the matching Deployment to `0`. Its tooltip
  clarifies that this reduces resources by scaling the thread container to zero.
- `Archive` uses the same scale-to-zero path and leaves room for DB archival metadata. Its tooltip
  describes the action as deep sleep for the thread container.
- `Delete (Delete Container)` removes the matching Kubernetes runtime resources, but does not touch
  GitHub PRs.
- `Merge with upstream` wakes the thread worker, then asks the Node.js task manager to fetch and
  merge the configured base branch and push the feature branch.
- `Make commit` wakes the thread worker, commits current workspace changes if there are any, and
  pushes the thread branch.
- `Open draft PR` wakes the worker and opens or reuses a GitHub PR only on demand. New PRs are
  created as drafts with `WIP - ...` titles and a body beginning with `WIP`.
- `Terminal` wakes the worker and opens `/dd-thread/<thread-short>/terminal` inline in the response
  panel for a shell inside the Node.js worker container.
