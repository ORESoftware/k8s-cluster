# dd-dev-server

Node.js + TypeScript HTTP server that lives inside a Docker container, keeps a warm checkout of the
configured git repo, and streams agent events back to the REST API and directly to the browser.

Public homepage rendering moved to `remote/deployments/web-home-rs`. This server is worker/API only.

The production shape is **one thread/conversation UUID maps to one container runtime and one git
branch** (`agent/k8s/openai-5.5/<uuid>/<slug>`). The container is pinned to a single threadId via
`REMOTE_DEV_THREAD_ID` on startup; subsequent prompts in that thread are queued into the same
runtime so workspace context is preserved across tasks. The server refuses to start without a
threadId — a per-thread pod is the only supported deployment shape.

This container is what `/u/admin/remote-dev` talks to. See [`../../readme.md`](../../readme.md) for the
big-picture architecture and
[`../../../docs/dev-hybrid-chat-plan-v4-k8s.md`](../../../docs/dev-hybrid-chat-plan-v4-k8s.md) for the
design rationale.

## Source layout

```
remote/deployments/dev-server/
├── Dockerfile                 # multi-stage build; bakes a warm configured repo + optional node_modules
├── .dockerignore
├── package.json               # fastify + zod + supabase-js, tsx for dev
├── tsconfig.json              # strict, NodeNext, output to dist/
└── src/
    ├── server.ts              # HTTP server, /tasks /stream /ws /terminal /cancel /thread/merge-upstream /thread/make-commit /thread/open-pr /healthz
    ├── token.ts               # HMAC verifier for direct browser → docker SSE
    ├── realtime.ts            # Supabase Broadcast publisher (per-user channel)
    ├── agents/
    │   ├── types.ts           # AgentRunner interface
    │   ├── index.ts           # selector + per-runner env allowlist
    │   ├── claude-cli.ts      # working: spawns `claude`
    │   ├── claude-sdk.ts      # working: @anthropic-ai/claude-agent-sdk
    │   ├── cluster-mcp.ts     # read-only cluster MCP config shared by SDK runners
    │   ├── openai-codex-cli.ts# working (after `codex` binary install)
    │   └── openai-sdk.ts      # working: @openai/agents
    └── storage/
        ├── types.ts           # StorageAdapter interface
        ├── index.ts           # selector that reads DEFAULT_STORAGE_PROVIDER
        ├── local.ts           # working: copies file + returns public URL
        ├── s3-r2.ts           # scaffolded; install @aws-sdk/client-s3 to activate
        ├── gcs.ts             # scaffolded; install @google-cloud/storage to activate
        └── drive.ts           # scaffolded; install googleapis to activate
```

## Build

The build can optionally receive a GitHub deploy key and `DD_REPO_URL` so the image can
pre-clone that repo at `--depth=1` and run `pnpm install`. Use BuildKit's `--secret` flag for the key, never
`--build-arg`, so the key never lands in any image layer. If `DD_REPO_URL` is omitted, the image
is a generic worker base and the container clones the configured repo at runtime.
The base package includes the NATS and WebSocket client dependencies used by queued worker fanout;
runtime repo, branch, and credentials are injected through Kubernetes env/secrets, not baked into
the image.

```bash
DOCKER_BUILDKIT=1 docker build \
  --build-arg DD_REPO_URL=git@github.com:org/repo.git \
  --build-arg DD_REPO_REF=dev \
  --secret id=github_deploy_key,src=$HOME/.ssh/dd_deploy \
  -t dd-dev-server:latest \
  remote/deployments/dev-server
```

Before the first build you need a `pnpm-lock.yaml` (the Dockerfile expects it):

```bash
cd remote/deployments/dev-server && pnpm install
```

## Run

```bash
docker run --rm -p 8080:8080 \
  --env-file ./.env.dev-server \
  dd-dev-server:latest
```

## Current EC2 runtime endpoint (verified May 15, 2026)

- Public homepage URL: `http://54.91.17.58/` and `http://54.91.17.58/home`
- Node worker/API routes are exposed behind the cluster gateway path rules.
- Ops URLs are exposed by the gateway with temporary dd header auth. Do not echo the configured
  value in public responses or docs.

Runtime split in the baseline Argo app:

- `dd-remote-web-home` (Rust, `/`, `/home`, `/agents/tasks`, `/agents/threads`)
- `dd-dev-server-api` (Node worker/API)
- `dd-remote-gateway` (path splitter on host port `80`)
- `dd-grafana` at `/telemetry/`
- `dd-prometheus` at `/prometheus/`
- `dd-nats` monitor/exporter at `/nats/` and `/nats-metrics/metrics`
- `dd-gleamlang-server` at `/gleam/home`, `/gleam/healthz`, and `/gleam/ws`
- `dd-gleam-mcp-server` at `/mcp`, `/mcp/home`, `/mcp/healthz`, and `/mcp/metrics`
- reaper/cron status surfaces at `/reaper/` and `/cron/`

`/healthz` is the only unauthenticated route on the Node API. Everything else requires either
`X-Server-Auth: $SERVER_AUTH_SECRET` (server-to-server, e.g. our Vercel routes) or — for
`GET /stream/:taskId` only — a short-lived HMAC-signed `?token=…` issued by Vercel's
`/api/admin/remote-dev/sign-token`.

`POST /thread/merge-upstream` is server-authenticated and runs inside the single UUID-pinned
worker. It verifies the workspace is on the feature branch, fetches `origin/$BASE_BRANCH` with a
shallow fetch, deepens only when Git needs more history, merges with
`git merge --no-edit origin/$BASE_BRANCH` (no rebase), and pushes the branch so the existing GitHub
PR updates.

`POST /thread/make-commit` is server-authenticated and runs inside the pinned worker. It stages all
workspace changes, creates a manual commit when the tree is dirty, and pushes the thread branch.
`GET /terminal` serves the browser terminal; `/terminal/ws` carries command input and output over
the gateway-proxied worker WebSocket.

The worker prepares Node dependencies only when the configured repo has a root `package.json`.
Non-Node repositories still get the same git checkout, thread branch, commit, PR, and terminal
workflow without a failing package install.

## Environment variables

> **All credentials are read from `process.env` at runtime.** No secrets are baked into the image.
> The image does bake git, OpenSSH, GitHub CLI, provider CLIs, the compiled server, and a warm
> configured repo template owned by the built-in `node` user. In
> production, the K8s `dd-agent-secrets` Secret (filled in from
> [`../../k8s/02-secrets.template.yaml`](../../k8s/02-secrets.template.yaml)) is consumed by every
> per-thread pod via `envFrom`. Local dev: pass via `--env-file` or `docker run -e`.

### Required — server core

| Var                  | Purpose                                                                                                                      |
| -------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `ANTHROPIC_API_KEY`  | Auth for the `claude` CLI the server spawns.                                                                                 |
| `SERVER_AUTH_SECRET` | Shared secret presented by Vercel in `X-Server-Auth`. Random, ≥ 32 chars.                                                    |
| `DD_REPO_URL`        | Git URL for the repo this thread container is pinned to. Required at runtime; optional at build time for a generic worker image. GitHub HTTPS URLs are converted to SSH at boot when `GH_DEPLOY_KEY` is present so branch pushes use the deploy key. |
| `GH_DEPLOY_KEY`      | OpenSSH private key for `git fetch` / `git push` against `DD_REPO_URL`. The server writes this to `~/.ssh/id_ed25519` at boot. |
| `GH_PAT`             | GitHub fine-grained token used by `gh pr create`. Scope it to the configured repo with Contents + Pull Requests. PR creation fails closed when this is unset. |

### Required — event ingestion

| Var                   | Purpose                                                                                                                                             |
| --------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| `EVENT_INGEST_URL`    | HTTP route that receives streamed events. In the EC2 cluster this is `http://dd-remote-rest-api.default.svc.cluster.local:8082/api/agents/events`.  |
| `EVENT_INGEST_SECRET` | Shared secret sent in `X-Agent-Auth`. In the EC2 cluster this is sourced from `SERVER_AUTH_SECRET`; Vercel may still use `REMOTE_DEV_INGEST_SECRET`. |

### Required — direct-stream HMAC tokens

| Var                       | Purpose                                                                                                                                                                             |
| ------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `REMOTE_DEV_TOKEN_SECRET` | HMAC-SHA256 secret for `?token=` validation on `GET /stream/:taskId`. **Must equal Vercel's `REMOTE_DEV_TOKEN_SECRET`** — the two sides verify each other's signatures. ≥ 32 chars. |

### Recommended — agent provider

The runner that drives each task is pluggable. Default is Generic AI SDK; it can be
overridden per dispatch (UI picker / API `provider` field) or globally via `AGENT_PROVIDER`.
Each task walks `AGENT_PROVIDER_ROTATION` and every configured key for that provider before moving
on. The default order is the generic AI SDK OpenAI-compatible pool (OpenCode Zen, DeepSeek,
Qwen/DashScope, xAI/Grok), then the dedicated OpenCode-compatible provider, then OpenAI SDK,
Claude SDK, and Gemini keys. Generic AI SDK and OpenCode receive bounded workspace
tools for repo inspection and file edits; Gemini remains model-only and is skipped for repo-edit
prompts. Simple append-to-file prompts such as `append "foobar" to todos.md`, and PR-only prompts
such as `make a pr`, are handled by deterministic paths before provider fallback.

| Provider           | Status            | Auth                                                      | Notes                                                                                            |
| ------------------ | ----------------- | --------------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| `gemini-sdk`       | working           | `GOOGLE_API_KEY` or `GEMINI_API_KEY` (+ optional `GEMINI_MODEL` / `GEMINI_FALLBACK_MODEL`) | Model-only response runner. It cannot edit the workspace; quota/rate-limit failures retry once with `gemini-3.1-flash-lite`. |
| `generic-ai-sdk`   | working           | Provider-specific JSON arrays such as `DEEPSEEK_API_KEYS_JSON`, `DASHSCOPE_API_KEYS_JSON`, and `XAI_API_KEYS_JSON` | Uses `ai` + `@ai-sdk/openai-compatible` against OpenCode Zen, DeepSeek V4, Qwen 3.6 Max, and Grok 4.x endpoints with bounded workspace tools for file inspection and edits. |
| `opencode-ai-sdk`  | working           | `OPENCODE_API_KEY` (+ optional `OPENCODE_MODELS` / `OPENCODE_BASE_URL`) | Uses `ai` + `@ai-sdk/openai-compatible` against OpenCode Zen with bounded workspace tools; default model order is `big-pickle`, `deepseek-v4-flash-free`, `minimax-m2.5-free`, `nemotron-3-super-free`, `qwen3.6-plus-free`. |
| `claude-sdk`       | working           | `ANTHROPIC_API_KEY`                                       | Uses `@anthropic-ai/claude-agent-sdk` with structured streaming and an explicit tool allowlist.  |
| `claude-cli`       | working           | `ANTHROPIC_API_KEY`                                       | Spawns the `claude` binary installed in the Dockerfile. Good fallback if SDK behavior regresses. |
| `openai-sdk`       | working           | `OPENAI_API_KEY` (+ optional `OPENAI_MODEL`)              | Uses `@openai/agents` with local shell/apply-patch tools scoped to the thread workspace. |
| `openai-codex-cli` | working           | `OPENAI_API_KEY` (+ optional `CODEX_MODEL`, e.g. `gpt-5.5`) | Spawns OpenAI's `codex` CLI installed in the Dockerfile and parses JSON/NDJSON output.         |

| Var                 | Purpose                                                                                                                     |
| ------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| `AGENT_PROVIDER`    | Default runner if the dispatch doesn't specify one. Defaults to `generic-ai-sdk`. One of `generic-ai-sdk` / `opencode-ai-sdk` / `gemini-sdk` / `claude-sdk` / `claude-cli` / `openai-sdk` / `openai-codex-cli`. |
| `AGENT_PROVIDER_ROTATION` | Comma/space separated retry order. Defaults to `generic-ai-sdk,opencode-ai-sdk,openai-sdk,claude-sdk,gemini-sdk`. The selected provider is appended if absent. |
| `AGENT_FALLBACK_PROVIDER` | First retry provider when the selected runner fails. Defaults to `generic-ai-sdk`.                                                        |
| `AGENT_SECONDARY_FALLBACK_PROVIDER` | Second retry provider when the selected runner or primary fallback fails. Defaults to `opencode-ai-sdk`.                           |
| `ANTHROPIC_API_KEY` | Required when provider is a `claude-*` runner.                                                                              |
| `ANTHROPIC_API_KEYS_JSON` | Optional JSON array of Anthropic keys. Tried before `ANTHROPIC_API_KEY`; `CLAUDE_API_KEYS_JSON` is also accepted.             |
| `ANTHROPIC_MODEL`   | Optional. Defaults to `claude-opus-4-7`; read by CLI/SDK when set.                                                          |
| `GOOGLE_API_KEY`    | Preferred API key for `gemini-sdk`; mapped into the runner's strict `GEMINI_API_KEY` allowlist.                              |
| `GEMINI_API_KEY`    | Alternate API key for `gemini-sdk` when `GOOGLE_API_KEY` is unset.                                                          |
| `GEMINI_API_KEYS_JSON` | Optional JSON array of Gemini keys. Tried before single-key env vars; `GOOGLE_API_KEYS_JSON` is also accepted.               |
| `GEMINI_MODEL`      | Optional. Defaults to `gemini-3.1-pro-preview`.                                                                             |
| `GEMINI_FALLBACK_MODEL` | Optional. Defaults to `gemini-3.1-flash-lite`; used once when the primary Gemini model returns a quota/rate-limit failure. |
| `OPENAI_API_KEY`    | Required when provider is an `openai-*` runner.                                                                             |
| `OPENAI_API_KEYS_JSON` | Optional JSON array of OpenAI keys. Tried before `OPENAI_API_KEY`.                                                         |
| `OPENAI_MODEL`      | Optional. Defaults to `gpt-5.5`; read by the SDK runner if set.                                                            |
| `CODEX_MODEL`       | Optional. Defaults to `OPENAI_MODEL`; pins `codex --model <name>` per dispatch.                                            |
| `OPENCODE_API_KEY`  | Optional direct OpenCode Zen key for `https://opencode.ai/zen/v1`. `opencode-ai-sdk` can also use DeepSeek, Qwen/DashScope, and xAI/Grok keys through the compatible source matrix. |
| `OPENCODE_API_KEYS_JSON` | Optional JSON array of OpenCode keys. Tried before `OPENCODE_API_KEY`; `OPENCODE_ZEN_API_KEYS_JSON` is also accepted.     |
| `OPENCODE_BASE_URL` | Optional. Defaults to `https://opencode.ai/zen/v1`.                                                                        |
| `OPENCODE_MODELS`   | Optional JSON array or comma/newline separated OpenCode model list. Defaults to five free/low-cost Zen models.             |
| `DEEPSEEK_API_KEYS_JSON` | Optional JSON array of DeepSeek keys. Tried before `DEEPSEEK_API_KEY` by `generic-ai-sdk`. Defaults to `deepseek-v4-flash,deepseek-v4-pro`. |
| `DASHSCOPE_API_KEYS_JSON` | Optional JSON array of Alibaba Model Studio / DashScope keys. `QWEN_API_KEYS_JSON` and `ALIBABA_API_KEYS_JSON` are also accepted. |
| `XAI_API_KEYS_JSON` | Optional JSON array of xAI keys. `GROK_API_KEYS_JSON` is also accepted. Defaults to `grok-4.3`. |
| `THREAD_CONTEXT_BASE_URL` | Optional. Defaults to the in-cluster REST API. Workers call `/api/agents/threads/:threadId/context` before each task. |
| `THREAD_CONTEXT_LIMIT` | Optional. Defaults to `20` prior tasks.                                                                                  |
| `THREAD_CONTEXT_MAX_CHARS` | Optional. Defaults to `48000` characters injected into the prompt.                                                   |
| `REPO_CONTEXT_MAX_CHARS` | Optional. Defaults to `24000` characters from `AGENTS.md`, `agents/*.md`, and `docs/*.md`.                            |
| `AGENT_OPTIMISTIC_MODE` | Optional. Defaults to enabled. Set to `false` only when agents should pause for clarification instead of making scoped assumptions. |
| `AGENT_MCP_URL` | Optional. In-cluster MCP endpoint passed to SDK runners, defaulting in Kubernetes to `http://dd-gleam-mcp-server.default.svc.cluster.local:8090/mcp`. |
| `AGENT_MCP_CONNECT_TIMEOUT_MS` | Optional. MCP connect timeout for SDK runners. Defaults to `3000`; clamped by the runner. |
| `AGENT_MCP_ENABLED` | Optional. Set to `false` to suppress MCP prompt context and runner MCP connection attempts. |

Each runner is given a **strict env allowlist** (`PATH`, `HOME`, `USER`, `LANG`, `NODE_ENV`, plus
its provider-specific API key). The agent never sees `GH_PAT`, `GH_DEPLOY_KEY`,
`SUPABASE_SERVICE_ROLE_KEY`, etc.

Every agent call receives a shared prompt wrapper before it reaches the selected SDK/CLI runner. The
wrapper includes the thread UUID, current task UUID, optimistic operating mode, repo-local context
from `AGENTS.md`/`agents/*.md`/`docs/*.md`, and the context rows the operator kept checked in the
thread UI. The picker can seed the prompt with durable Postgres context blobs, previous thread
tasks, and individual breadcrumbs from `agent_remote_dev_breadcrumbs`; unchecked rows are omitted
from the worker payload. Breadcrumbs are no longer fetched as an automatic tail by the worker.

When `AGENT_MCP_URL` is set, the worker injects a short runtime context section into every task
prompt. The OpenAI SDK runner connects the endpoint as MCP server `dd_cluster`; the Claude SDK runner
declares the same server and allows its read-only tools. CLI runners still get the prompt hint, but
their native MCP support depends on the installed CLI.

### Recommended — Supabase Realtime fan-out

The browser subscribes to a per-user Supabase Broadcast channel (`remote-dev:user:<ddUserId>`) for
live updates. This path is intentionally lambda-independent — once the page is loaded, the
WebSocket lives entirely between the browser and Supabase. The docker writes to that channel with
the service role key.

| Var                                            | Purpose                                                                                                                 |
| ---------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| `SUPABASE_URL` (or `NEXT_PUBLIC_SUPABASE_URL`) | Supabase project URL.                                                                                                   |
| `SUPABASE_SERVICE_ROLE_KEY`                    | Service role key — required for the docker to broadcast on behalf of any user. **Never** put this in browser-bound env. |

If unset, the docker silently skips the Realtime publish (events still flow through
`EVENT_INGEST_URL` → NeonDB; the browser falls back to the 20s polling loop).

### Recommended — heartbeat → Vercel

Every `HEARTBEAT_INTERVAL_MS` the docker POSTs a snapshot of in-flight tasks to Vercel. Vercel
caches the most recent receipt in Redis (90s TTL); the UI's `/api/admin/remote-dev/docker-health`
route reports "alive" if either a fresh heartbeat or a live `/healthz` ping succeeds.

| Var                     | Purpose                                                                                      |
| ----------------------- | -------------------------------------------------------------------------------------------- |
| `HEARTBEAT_URL`         | e.g. `https://<your-vercel-app>/api/admin/remote-dev/heartbeat`.                             |
| `HEARTBEAT_SECRET`      | Shared secret sent in `X-Heartbeat-Auth`. Must equal Vercel's `REMOTE_DEV_HEARTBEAT_SECRET`. |
| `HEARTBEAT_INTERVAL_MS` | Default `20000` (20s). Clamped to ≥ 5s.                                                      |

### Optional — paths / behaviour

| Var                           | Default                         | Purpose                                                                                                                               |
| ----------------------------- | ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| `PORT`                        | `8080`                          | HTTP port.                                                                                                                            |
| `HOST`                        | `0.0.0.0`                       | Bind host.                                                                                                                            |
| `BASE_BRANCH`                 | `dev`                           | Branch to fetch the thread workspace from.                                                                                            |
| `WORKSPACE_REPO`              | `/home/node/workspace/repo`     | Persistent thread workspace — seeded from `/home/node/repo-template` on cold boot, then `git fetch`ed each subsequent boot.           |
| `OUTPUTS_DIR`                 | `/home/node/workspace/outputs`  | Where the agent writes publishable files (markdown, PDF, etc.); scanned + uploaded after the agent run exits.                         |
| `REMOTE_DEV_THREAD_ID`        | unset                           | **Required.** The UUID created by `/u/admin/remote-dev` for the conversation. The container is pinned to one thread for its lifetime. |
| `REMOTE_DEV_THREAD_TITLE`     | unset                           | Optional first-task title used to boot the thread worker directly onto the prompt-derived feature branch instead of a UUID placeholder branch. |
| `IDLE_TIMEOUT_MS`             | `1800000`                       | In-process idle watchdog. For k8s thread pods we set this to `0` and let the control-plane reaper scale Deployment replicas to 0/1.   |
| `ENTRYPOINT_INSTALL_DEPS`     | `false`                         | Set to `true` only when the entrypoint should run `pnpm install` before the server starts. Default defers dependency install until the server has prepared the feature branch, avoiding warm-boot base-branch resets. |
| `THREAD_BREADCRUMB_WRITE_TIMEOUT_MS`| `5000`                     | Per-call timeout for fire-and-forget breadcrumb POSTs to rest-api.                                                                   |
| `DEFAULT_STORAGE_PROVIDER`    | `local`                         | One of `local` / `s3` / `r2` / `gcs` / `drive`.                                                                                       |
| `GH_DEPLOY_KEY_PATH`          | `/home/node/.ssh/id_ed25519`    | Where `GH_DEPLOY_KEY` is materialised at boot.                                                                                        |
| `GIT_AUTHOR_NAME`             | `DD Agent`                      | Commit author.                                                                                                                        |
| `GIT_AUTHOR_EMAIL`            | `agent@dancingdragons.dev`      | Commit email.                                                                                                                         |
| `OTEL_SERVICE_NAME`           | `dd-dev-server-api`             | Service name attached to explicit OTLP spans.                                                                                         |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | unset                           | OTLP/HTTP collector base URL, e.g. `http://dd-otel-collector.observability.svc.cluster.local:4318`.                                   |

Telemetry is explicit only: no OpenTelemetry auto-instrumentation package, require hook, fetch/http
monkey-patch, or framework patching is used. The server exports direct OTLP/HTTP trace payloads
from `src/telemetry.ts` and exposes Prometheus metrics at `/metrics`.

### Storage providers — set the block matching your `DEFAULT_STORAGE_PROVIDER`

#### `local` (dev / smoke testing only)

| Var                             | Purpose                                                                  |
| ------------------------------- | ------------------------------------------------------------------------ |
| `LOCAL_STORAGE_ROOT`            | Directory to copy files into. Default `/home/node/workspace/published`. |
| `LOCAL_STORAGE_PUBLIC_BASE_URL` | URL the browser uses to fetch from `LOCAL_STORAGE_ROOT`. **Required.**   |

#### `s3` (AWS S3)

> **Not yet wired.** The adapter is scaffolded; install `@aws-sdk/client-s3` and replace the TODO
> block in [`src/storage/s3-r2.ts`](src/storage/s3-r2.ts) before this works.

| Var                    | Purpose                                                                                                                            |
| ---------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `S3_BUCKET`            | Target bucket.                                                                                                                     |
| `S3_REGION`            | e.g. `us-east-1` (default).                                                                                                        |
| `S3_ACCESS_KEY_ID`     | IAM access key.                                                                                                                    |
| `S3_SECRET_ACCESS_KEY` | IAM secret.                                                                                                                        |
| `S3_PUBLIC_BASE_URL`   | CDN / public URL prefix that maps `<base>/<key>` to the object. **Required** — the adapter refuses to emit a URL nobody can fetch. |

#### `r2` (Cloudflare R2 — S3-compatible)

> Same status as `s3`: scaffolded, awaiting SDK install.

| Var                    | Purpose                                            |
| ---------------------- | -------------------------------------------------- |
| `R2_BUCKET`            | Target bucket.                                     |
| `R2_ENDPOINT`          | e.g. `https://<account>.r2.cloudflarestorage.com`. |
| `R2_ACCESS_KEY_ID`     | R2 access key.                                     |
| `R2_SECRET_ACCESS_KEY` | R2 secret.                                         |
| `R2_REGION`            | Default `auto`.                                    |
| `R2_PUBLIC_BASE_URL`   | Public Worker / `pub-…r2.dev` prefix.              |

#### `gcs` (Google Cloud Storage)

> Stubbed. Install `@google-cloud/storage` and wire the upload in
> [`src/storage/gcs.ts`](src/storage/gcs.ts).

| Var                   | Purpose                                                            |
| --------------------- | ------------------------------------------------------------------ |
| `GCS_PROJECT_ID`      | GCP project id.                                                    |
| `GCS_BUCKET`          | Target bucket.                                                     |
| `GCS_KEY_JSON_BASE64` | Base64-encoded service-account JSON.                               |
| `GCS_PUBLIC_BASE_URL` | e.g. `https://storage.googleapis.com/<bucket>` for public buckets. |

#### `drive` (Google Drive)

> Stubbed. Install `googleapis` and wire the upload in
> [`src/storage/drive.ts`](src/storage/drive.ts).

| Var                     | Purpose                                                             |
| ----------------------- | ------------------------------------------------------------------- |
| `DRIVE_FOLDER_ID`       | Parent folder in Drive. The service account must have Editor on it. |
| `DRIVE_KEY_JSON_BASE64` | Base64-encoded service-account JSON.                                |
| `DRIVE_SHARE_MODE`      | `anyone` (default) / `domain` / `private`.                          |

## API surface

| Method | Path                    | Auth                              | Notes                                                                                                                                                                           |
| ------ | ----------------------- | --------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `GET`  | `/healthz`              | none                              | Liveness — returns `{ ok, startedAt, inFlightCount, totalTracked }`.                                                                                                            |
| `GET`  | `/metrics`              | none                              | Prometheus metrics scraped by the OpenTelemetry Collector.                                                                                                                      |
| `GET`  | `/tasks`                | `X-Server-Auth`                   | Snapshot of every task in memory (used by Vercel to merge with NeonDB on first load).                                                                                           |
| `POST` | `/tasks`                | `X-Server-Auth`                   | Body `{ taskId, prompt, repo?, baseBranch?, userId?, threadId?, provider?, branch?, threadTitle? }`. Queues the run into the thread workspace, returns `queuedBehind` / `queuePosition`, and rejects a different repo/base branch than the container is pinned to. |
| `GET`  | `/stream/:taskId`       | `X-Server-Auth` **or** `?token=…` | Server-Sent Events. `Last-Event-ID` resumes.                                                                                                                                    |
| `GET`  | `/ws`                   | `X-Server-Auth`                   | Worker WebSocket for the pinned thread. Use `/dd-thread/<short>/ws?threadId=<uuid>&taskId=<uuid>` through the gateway; it replays task events and streams new worker events faster than the NATS/Gleam fanout path. |
| `GET`  | `/terminal`             | `X-Server-Auth`                   | Browser terminal for the pinned worker. Use `/dd-thread/<short>/terminal?threadId=<uuid>` through the gateway.                                                                  |
| `POST` | `/thread/merge-upstream` | `X-Server-Auth`                  | Merges `origin/$BASE_BRANCH` into the pinned thread branch and pushes.                                                                                                          |
| `POST` | `/thread/make-commit`   | `X-Server-Auth`                   | Stages current workspace changes, commits if dirty, and pushes the pinned thread branch.                                                                                        |
| `POST` | `/thread/open-pr`       | `X-Server-Auth`                   | Explicitly opens or reuses a draft WIP PR for the pinned thread branch. Normal tasks do not create PRs.                                                                         |
| `POST` | `/tasks/:taskId/cancel` | `X-Server-Auth`                   | SIGTERMs the child, emits `done` with `cancelled`.                                                                                                                              |

The HMAC token format used by `?token=` is documented in [`src/token.ts`](src/token.ts) — both
sides share `REMOTE_DEV_TOKEN_SECRET`.

## Thread/task lifecycle

For each new thread, the container/workspace:

1. Runtime clone or baked-template clone uses `git clone --depth=1 --branch <BASE_BRANCH>`.
   Warm boots only refresh `origin/<BASE_BRANCH>` with a depth-1 fetch; the entrypoint does not
   detach or reset a reused workspace back to the parent branch.
2. Choose the session branch. If dispatch provides `branch`, reuse it; otherwise derive one as
   `agent/k8s/openai-5.5/<threadId>/<slugified-thread-title>`. If that remote branch already
   exists, fetch it shallowly and switch from it; otherwise create the feature branch from
   `origin/<BASE_BRANCH>`. If a reused workspace is still on the parent branch, the worker fails
   closed instead of running a task on `BASE_BRANCH`.
3. Install repo dependencies only after the feature branch is prepared. Dependency/cache artifacts
   are then restored/cleaned with generated dirs excluded.
4. Start listening after the thread workspace is ready in `thread` mode.

For each `POST /tasks` in that thread:

1. Add the task to the in-process per-session promise queue. If another task is running or queued,
   emit `queued-behind-active-task` with `queueDepth`, `queuePosition`, and blocker task IDs. That
   status goes through the normal event pipeline: thread log, metrics, NATS fanout, worker
   WebSocket fanout, and SSE replay.
2. When the task reaches the front of the queue, assert the workspace is on the feature branch.
   If it is on the parent branch or detached, fail the task instead of continuing.
3. Fetch `origin/<BASE_BRANCH>` with `--depth=1`, deepen only if Git cannot find a merge base, then
   merge `origin/<BASE_BRANCH>` into the feature branch before the user task starts. If conflicts
   occur, a workspace-capable provider gets a pre-task conflict-resolution prompt; unresolved
   conflicts abort the task.
4. POST prompt/event/lifecycle breadcrumbs to rest-api (persisted in Postgres
   `agent_remote_dev_breadcrumbs`; see `remote/libs/interfaces/redis` for the cross-runtime
   shape and the optional Redis cache key conventions).
5. `mkdir -p $OUTPUTS_DIR/<taskId>` so the agent has a place to write.
6. Build the shared context prompt from repo files, selected Postgres context rows, selected
   previous-task rows, and selected breadcrumb rows.
7. Apply any supported deterministic workspace edit, otherwise run the selected provider
   (`openai-sdk` by default, with Claude/Gemini overrides available).
8. Stage workspace changes while excluding generated dependency/cache dirs, then commit and push `origin <session-branch>`.
9. **Walk `$OUTPUTS_DIR/<taskId>/`** — every regular file (one level deep) is uploaded via the
   configured storage adapter, emitting one `artifact` event per file with the resulting URL.
10. Emit terminal `done` event.

PR creation is intentionally separate. The UI must call `/thread/open-pr` (through the Rust REST
API or Next.js admin API) when the operator wants a PR. New PRs are always created with `--draft`,
a `WIP - ...` title, and a body that starts with `WIP`; existing PRs are reused.

Tasks are GC'd from memory one hour after completion. The container owns its single workspace for
the lifetime of the pod — sleep/wake of the surrounding K8s Deployment preserves the PVC, so the
workspace remounts intact on the next dispatch.

### UUID reuse behavior (validated)

Reusing the same `threadId` on multiple `/tasks` calls reuses the same session and branch. Example
host-side validation:

- thread UUID: `00000000-0000-4000-8000-000000000001`
- task IDs: `11111111-1111-4111-8111-111111111111`, `22222222-2222-4222-8222-222222222222`
- both task submissions were accepted and mapped to the same branch/session

## Next.js relay and persistence contract

The frontend should call Next.js first (`/api/admin/remote-dev/dispatch`), not the worker directly.
Next.js records task state in NeonDB, then forwards to the worker using `X-Server-Auth`.

The worker itself does not use Drizzle and does not write SQL directly. It reports events/status
by:

- POSTing event payloads to `EVENT_INGEST_URL` so either the Rust REST API or Next.js persists to
  NeonDB tables (`agent_remote_dev_threads`, `_tasks`, `_events`, `_artifacts`)
- broadcasting live updates to Supabase channels (`remote-dev:user:<ddUserId>`)

PR URLs emitted by explicit `pr_open` events are surfaced on `/u/admin/remote-dev`,
`/agents/tasks`, and `/agents/threads`.

## Smoke test

After the container is running:

```bash
# direct from the host (uses X-Server-Auth)
curl -fsS http://localhost:8080/healthz   # → {"ok":true}

curl -X POST http://localhost:8080/tasks \
  -H "X-Server-Auth: $SERVER_AUTH_SECRET" \
  -H 'Content-Type: application/json' \
  -d '{"prompt":"Say hi from the agent"}'

# tail the stream
curl -N http://localhost:8080/stream/<taskId> \
  -H "X-Server-Auth: $SERVER_AUTH_SECRET"
```

## Deploying

The production shape is a **per-thread K8s pod** managed by the v4 manifests in
[`remote/k8s/`](../../k8s/) — one container pinned to one chat / threadId from `/u/admin/remote-dev`.
The orchestrator
([`src/lib/server/remote-dev/container-registry.ts`](../../../src/lib/server/remote-dev/container-registry.ts))
instantiates the per-thread Deployment + Service + PVC + Ingress on first dispatch and tears them
down when the thread is ended.

For k8s pods, sleep/wake is managed outside the container by the control-plane reaper
(`/api/admin/remote-dev/reaper/sweep`) scaling Deployment replicas `1 -> 0 -> 1`. The pod env sets
`IDLE_TIMEOUT_MS=0` to avoid self-terminating loops.

This folder is the AWS EC2 Kubernetes variant. Use it for the ECR image and the per-thread pods
created by the v4 manifests in `remote/k8s/`. Laptop/minikube iteration belongs in
[`../../dev-server-local`](../../dev-server-local/), which carries its own local manifest and headless
smoke tests. Direct `docker run` from this folder is still useful for isolated debugging, but it is
not the local cluster path.

The ECR refresh workflow lives at
[`../../../.github/workflows/remote-dev-server-ecr.yml`](../../../.github/workflows/remote-dev-server-ecr.yml).
It rebuilds from `dev` at 4am America/New_York every three days, pushing both `latest` and the
commit SHA. Configure `AWS_ROLE_TO_ASSUME`, `AWS_REGION`, `REMOTE_DEV_ECR_REPOSITORY`, and
`DD_REPO_DEPLOY_KEY` in GitHub Secrets/Variables.
