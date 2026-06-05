# `remote/deployments/web-home-rs`

Rust public web layer for remote-dev.

## Purpose

- serves `GET /` and `GET /home` as the operator-facing homepage
- serves `GET /agents/tasks` as the cluster-hosted remote-dev diagnostics table
- serves `GET /agents/threads` as the thread-first chat/task UI with stored response events
- serves `GET /presence-test` as a 1-user, N-conversation in-browser harness for
  the gleamlang-presence-server (opens 1 user-scoped ws + N conv-scoped ws's,
  exposes join/leave/broadcast/device-logout controls, intended to be opened
  in 3 tabs as `alice/d1`, `bob/d2`, `carol/d3` to exercise cross-tab fan-out)
- serves `GET /wss-test` as a same-origin gateway WebSocket harness with presets
  for `dd-gleamlang-server`, `dd-webrtc-signaling-rs`, `gms/gcs/chat.vibe`,
  and the F# Rx burst endpoint; the page includes health checks, burst send,
  interval send, and separate sent/received counters for browser-side smoke tests.
  The page is public, but its service presets call gateway-authenticated upstream paths.
- keeps HTML/public pages out of the Node.js worker runtime
- keeps database credentials out of the public HTML server
- exposes `GET /healthz` for web liveness
- exposes `GET /metrics` for Prometheus/OpenTelemetry Collector scraping
- links to the Rust PIN auth service at `/auth?return=/home`
- loads live managed service Pods/containers from `/bastion/runtime/deployments`, with timed polling
  and Rust/Gleam websocket-triggered refreshes when Kubernetes runtime events arrive
- opens managed service container terminals inline through `/bastion/terminal`

Task dispatch / stream / cancel routing should use the per-thread Kubernetes Ingress path
(`/dd-thread/<short>/...`). Node.js is not the UUID router; each Node.js worker container is pinned
to exactly one thread and should only accept tasks for that thread.

## Agent tasks data

`/agents/tasks` and `/agents/threads` are SSR shells rendered by the Rust web layer with Maud. Each
page has same-origin CSS, JavaScript, and HTML-fragment bundle endpoints under `/assets/web-home/`.
Their browser JavaScript calls the same-origin gateway routes `GET /api/agents/tasks?limit=...` and
`GET /api/agents/tasks/:taskId/events?limit=...` directly. The gateway requires the operator
`dd_auth` cookie or legacy `Auth` header before forwarding those JSON routes to the REST API.

The REST API owns RDS/Postgres, Supabase fallback, and later NATS/Postgres event write paths. This
keeps page rendering separate from data access without making the webserver a proxy.

## Lambda Functions

`/lambdas/functions` is the Rust-served operator UI for stored lambda definitions. It calls
`/api/lambdas/functions` for CRUD and invokes saved functions through the gateway's
`POST /lambdas/invoke/<function-id>` route.

The editor exposes a deployment profile layer above the persisted lambda runtime: direct `nodejs`
and `python3` profiles, containerized `ruby`, `bash`, `golang`, `dart`, `erlang`, `elixir`, and `java`
profiles, plus `rust` and `gleamlang` process profiles that generate a Node.js wrapper using the
lambda runner's `context.containerPool.dispatch(...)` helper. The UI also
captures the intended base image and container runner (`containerd / ctr`, `containerd / nerdctl`,
or `docker`) in `metaData.lambdaDeployment` so operators can see and revise the deployment intent
without widening the REST API's trusted entry-command contract.

The page accepts query params to prefill a new draft. Common params are `slug`, `name` or
`displayName`, `description`, `status`, `runtime`, `processProfile` (`nodejs`, `python3`, `ruby`,
`bash`, `golang`, `dart`, `erlang`, `elixir`, `java`, `rust`, or `gleamlang`), `containerized`,
`containerRunner`, `baseImage`, `reuseKey`,
`idleTimeoutSeconds`, `maxRunMs`, `body` or `functionBody`, `request`, `labels`, `meta`, and
`containerPoolTimeoutMs`. JSON-valued params such as `request`, `labels`, and `meta` should be URL
encoded.

The Maud-rendered editor keeps user edits stable across background refreshes, highlights code in
the selected process profile, reports field-specific save validation, and requires the authenticated
`POST /lambdas/check` runner path to compile or syntax-check the draft before saving. The runner
applies the same managed runtime and host/container policy used for invocation, so containerized
Python, Ruby, Bash, Go, Dart, Erlang, Elixir, and Java drafts are checked in their runtime image.
Each language template includes the expected handler signature. Process-profile switches persist
the outgoing language body to localStorage and the root service worker cache, then restore the
incoming language's saved draft for the current function.

## Service directory

`/home` lists the managed runtime deployments including Solana contracts, VPN, live-mutex, bastion,
Redis, live-mutex load testing, trading, websocket services, load generators, and the container
pool. Its live containers table calls the authenticated bastion route
`/bastion/runtime/deployments`; Rust/Gleam websocket events trigger near-real-time reloads, with a
timed poll as the fallback. Terminal buttons embed
`/bastion/terminal?...` in the current page so service-container shells flow through the jumphost
instead of the public homepage process.

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

A `container: <state>` pill in the workspace top bar (next to the `Diagnostics table` and
`Service directory` links) polls `GET /api/agents/threads/:threadId/runtime` every 10 seconds
whenever a thread is selected. It normalises the Kubernetes view into a short lifecycle label:
`never-lived` / `non-existent` (no Deployment), `cold-start` / `warming` (Deployment exists but
Pod is still pulling or initializing), `starting`, `running` (Deployment ready with available
replicas), `suspended` (`desiredReplicas == 0` after sleep/archive), `dead (<reason>)` for
`CrashLoopBackOff` / `ImagePullBackOff` / non-zero exits, and `pending (<reason>)` when the
scheduler has not placed the Pod yet. The tooltip includes the underlying Pod/container detail.
The pill is also a button (`role="button"`, focusable, click / Enter / Space) so the operator can
force a runtime probe immediately without waiting for the next poll tick; the pill flips to
`container: probing` with a progress cursor while the manual probe is in flight.

The pill is hardened against the obvious foot-guns: each fetch runs through an `AbortController`
with a 15s timeout, so a slow or hung REST API does not pin the browser tab; a monotonically
incremented request token guards against stale responses overwriting fresh data after the user
switches threads; rapid clicks are coalesced through a 500ms debounce, and a manual click cancels
the next scheduled auto-poll up front so the two cannot race and abort each other; network errors,
non-2xx responses, and non-JSON bodies each flip the pill to a `container: probe error` / `probe
failed (<status>)` / `invalid response` red state with a `Click to retry.` tooltip (capped to 200
characters, whitespace collapsed, with a `(N consecutive failures)` suffix once N > 1); consecutive
failures back off the poll cadence (5s, 10s, 20s, 40s, capped at 60s); the poll cadence also slows
to 60s while the tab is hidden via `document.visibilityState`, then immediately re-probes when the
tab becomes visible again. Auto-polls keep the previous resolved label visible while their fetch is
in flight, so the pill only flashes `container: probing` for manual probes and the very first probe
after thread selection — that keeps `aria-live="polite"` quiet for screen readers and avoids a
visual flicker every 10s. `aria-busy` toggles to `true` while a probe is in flight, `aria-disabled`
tracks whether a thread is currently selected, and the classifier filters out `null` / non-object
entries from `pods`, `pod.conditions`, `pod.initContainers`, and `pod.containers` before walking
them so a malformed upstream payload cannot crash the page.
Thread Control expands while creating a new worker, then docks into a collapsed sticky bottom sheet
once the response stream takes over the middle column. The middle column owns response scrolling,
while the left thread list and right task list keep their independent sidebar scrolls.
Existing-thread-only controls stay hidden until the selected UUID is backed by a stored thread.

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
- `wss://<host>/admin/webrtc/runtime/ws?threadId=<uuid>&taskId=<uuid>` for Rust runtime fanout,
  including direct REST API status broadcasts when NATS event fanout is unhealthy.
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
- `Merge with siblings` creates a normal worker task that asks the selected thread branch to fetch
  and semantically merge sibling feature branches from other threads with the same repo and base
  branch, then commit and push the current branch.
- `Make commit` wakes the thread worker, commits current workspace changes if there are any, and
  pushes the thread branch.
- `Open draft PR` wakes the worker and opens or reuses a GitHub PR only on demand. New PRs are
  created as drafts with `WIP - ...` titles and a body beginning with `WIP`.
- `Terminal` wakes the worker and opens `/dd-thread/<thread-short>/terminal` inline in the response
  panel for a shell inside the Node.js worker container.
