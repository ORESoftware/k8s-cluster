# dd-dev-server-local

Node.js + TypeScript HTTP server that lives inside a Docker container, keeps a warm checkout of
`dd-next-1`, and streams agent events back to our Vercel app and directly to the browser.

This folder is the **local minikube** variant. Keep production EC2 Kubernetes work in
`remote/dev-server` and `remote/k8s`; keep laptop cluster iteration here. The local container is
still pinned to one thread via `REMOTE_DEV_THREAD_ID`, but the minikube manifest uses a single
reusable local thread id so you can verify the chat/runtime loop without creating production
per-thread stacks.

This container is what `/u/admin/remote-dev` talks to. See [`../readme.md`](../readme.md) for the
big-picture architecture and
[`../../docs/dev-hybrid-chat-plan-v3.md`](../../docs/dev-hybrid-chat-plan-v3.md) for the design
rationale.

## Local minikube quickstart

Prerequisites on the laptop:

```bash
brew install minikube
pnpm run minikube:preflight
pnpm run minikube:start
kubectl config use-context minikube
```

`minikube:start` defaults to a laptop-friendly Docker profile (`DD_MINIKUBE_CPUS=4`,
`DD_MINIKUBE_MEMORY=6144`, `DD_MINIKUBE_DISK_SIZE=40g`). Override those env vars when Docker
Desktop has more memory available. `minikube:preflight` verifies `minikube`, `kubectl`, and Docker
before startup; if it cannot connect to `~/.docker/run/docker.sock`, start Docker Desktop before
running the minikube scripts.

Install package dependencies once:

```bash
pnpm install
```

Build the image directly into minikube, apply the local manifests, and forward the service:

```bash
export GH_DEPLOY_KEY_PATH="${GH_DEPLOY_KEY_PATH:-$HOME/.ssh/id_ed25519}"
pnpm run minikube:build
pnpm run minikube:apply
pnpm run minikube:port-forward
```

The manifest at [`k8s/minikube-dev-server.yaml`](k8s/minikube-dev-server.yaml) contains placeholder
secrets. Replace `SERVER_AUTH_SECRET`, `ANTHROPIC_API_KEY`, `GH_PAT`, and `GH_DEPLOY_KEY` before
expecting a real Claude run to fetch, commit, push, and open a PR. The local image uses
`imagePullPolicy: Never`, so `pnpm run minikube:build` must run after changing this package. The
build script passes `GH_DEPLOY_KEY_PATH` to BuildKit as `github_deploy_key`; point it at a readable
deploy key with access to `dancing-dragons/dd-next-1`.

The local manifest mirrors the EC2 Kubernetes security boundaries that are useful during laptop
iteration: a dedicated `ServiceAccount`, namespace pod-security labels, a `ResourceQuota`, container
ephemeral-storage limits, and a `NetworkPolicy` that only exposes the HTTP port while allowing DNS,
HTTPS, and SSH egress. NetworkPolicy enforcement depends on the active minikube CNI; the manifest
still declares the boundary so clusters with enforcement enabled behave like the EC2 path.

Smoke-test the minikube manifest and local thread lifecycle without real external services:

```bash
pnpm run test:local-smoke
```

Or run the checks individually while debugging:

```bash
pnpm run test:minikube-manifest
pnpm run test:thread-smoke
pnpm run test:thread-ui-smoke
```

The manifest smoke test uses `kubectl create --dry-run=client` against the local minikube YAML. The
thread tests start the server on a random localhost port, put fake `git`, `gh`, `pnpm`, and
`claude` binaries first on `PATH`, dispatch a Claude-style prompt to `/tasks`, and waits for the
task to finish. It is intentionally headless and does not open browser windows.

If `pnpm run minikube:preflight` reports that Docker is installed but its daemon is not running,
start Docker Desktop and rerun `pnpm run minikube:start`. The offline smoke tests can still pass in
that state, but `minikube start`, image builds, and `kubectl` dry-runs against the live minikube
API will stay blocked until Docker's socket is reachable.

## Source layout

```
remote/dev-server-local/
├── Dockerfile                 # multi-stage build; bakes a warm dd-next-1 + node_modules
├── .dockerignore
├── package.json               # fastify + zod + supabase-js, tsx for dev
├── tsconfig.json              # strict, NodeNext, output to dist/
└── src/
    ├── server.ts              # HTTP server, /tasks /stream /cancel /healthz
    ├── token.ts               # HMAC verifier for direct browser → docker SSE
    ├── realtime.ts            # Supabase Broadcast publisher (per-user channel)
    ├── agents/
    │   ├── types.ts           # AgentRunner interface
    │   ├── index.ts           # selector + per-runner env allowlist
    │   ├── claude-cli.ts      # working: spawns `claude` (default)
    │   ├── claude-sdk.ts      # working: @anthropic-ai/claude-agent-sdk
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

## Local Docker Build

The build needs a GitHub deploy key with read access to `dancing-dragons/dd-next-1` so the image
can pre-clone the repo and run `pnpm install`. Use BuildKit's `--secret` flag — never `--build-arg`
— so the key never lands in any image layer. For local minikube iteration, build from this folder
and tag the image as `dd-dev-server-local:latest` so the minikube manifest can use
`imagePullPolicy: Never`.

```bash
DOCKER_BUILDKIT=1 docker build \
  --build-arg DD_REPO_URL=git@github.com:dancing-dragons/dd-next-1.git \
  --build-arg DD_REPO_REF=dev \
  --secret id=github_deploy_key,src=$HOME/.ssh/dd_deploy \
  -t dd-dev-server-local:latest \
  remote/dev-server-local
```

Before the first build you need a `pnpm-lock.yaml` (the Dockerfile expects it):

```bash
cd remote/dev-server-local && pnpm install
```

## Local Container Run

```bash
docker run --rm -p 8080:8080 \
  --env-file ./.env.dev-server \
  dd-dev-server-local:latest
```

`/healthz` is the only unauthenticated route. Everything else requires either
`X-Server-Auth: $SERVER_AUTH_SECRET` (server-to-server, e.g. our Vercel routes) or — for
`GET /stream/:taskId` only — a short-lived HMAC-signed `?token=…` issued by Vercel's
`/api/admin/remote-dev/sign-token`.

## Environment variables

> **All credentials are read from `process.env` at runtime.** Nothing is baked into the image. In
> production, the K8s `dd-agent-secrets` Secret (filled in from
> [`../k8s/02-secrets.template.yaml`](../k8s/02-secrets.template.yaml)) is consumed by every
> per-thread pod via `envFrom`. Local dev: pass via `--env-file` or `docker run -e`.

### Required — server core

| Var                  | Purpose                                                                                                                      |
| -------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `ANTHROPIC_API_KEY`  | Auth for the `claude` CLI the server spawns.                                                                                 |
| `SERVER_AUTH_SECRET` | Shared secret presented by Vercel in `X-Server-Auth`. Random, ≥ 32 chars.                                                    |
| `GH_DEPLOY_KEY`      | OpenSSH private key for `git fetch` / `git push` against `dd-next-1`. The server writes this to `~/.ssh/id_ed25519` at boot. |
| `GH_PAT`             | GitHub fine-grained token used by `gh pr create`. Scope: Contents + Pull Requests, `dancing-dragons/dd-next-1` only.         |

### Required — Vercel ingestion

| Var                   | Purpose                                                                                                     |
| --------------------- | ----------------------------------------------------------------------------------------------------------- |
| `EVENT_INGEST_URL`    | Vercel route that receives streamed events. Set to `https://<your-vercel-app>/api/admin/remote-dev/events`. |
| `EVENT_INGEST_SECRET` | Shared secret sent in `X-Agent-Auth`. Must equal Vercel's `REMOTE_DEV_INGEST_SECRET`.                       |

### Required — direct-stream HMAC tokens

| Var                       | Purpose                                                                                                                                                                             |
| ------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `REMOTE_DEV_TOKEN_SECRET` | HMAC-SHA256 secret for `?token=` validation on `GET /stream/:taskId`. **Must equal Vercel's `REMOTE_DEV_TOKEN_SECRET`** — the two sides verify each other's signatures. ≥ 32 chars. |

### Recommended — agent provider

The runner that drives each task is pluggable. Default is Claude Code via the `claude` CLI; can be
overridden per dispatch (UI picker / API `provider` field) or globally via `AGENT_PROVIDER`.

| Provider           | Status            | Auth                                                      | Notes                                                                                            |
| ------------------ | ----------------- | --------------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| `claude-sdk`       | working (default) | `ANTHROPIC_API_KEY`                                       | Uses `@anthropic-ai/claude-agent-sdk` with structured streaming and an explicit tool allowlist.  |
| `claude-cli`       | working           | `ANTHROPIC_API_KEY`                                       | Spawns the `claude` binary installed in the Dockerfile. Good fallback if SDK behavior regresses. |
| `openai-sdk`       | working           | `OPENAI_API_KEY` (+ optional `OPENAI_MODEL`)              | Uses `@openai/agents` with local shell/apply-patch tools scoped to the thread workspace.         |
| `openai-codex-cli` | working           | `OPENAI_API_KEY` (+ optional `CODEX_MODEL`, e.g. `gpt-5`) | Spawns OpenAI's `codex` CLI installed in the Dockerfile and parses JSON/NDJSON output.           |

| Var                 | Purpose                                                                                                                     |
| ------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| `AGENT_PROVIDER`    | Default runner if the dispatch doesn't specify one. One of `claude-sdk` / `claude-cli` / `openai-sdk` / `openai-codex-cli`. |
| `ANTHROPIC_API_KEY` | Required when provider is a `claude-*` runner.                                                                              |
| `OPENAI_API_KEY`    | Required when provider is an `openai-*` runner.                                                                             |
| `CODEX_MODEL`       | Optional. Pins `codex --model <name>` per dispatch.                                                                         |
| `OPENAI_MODEL`      | Optional. Read by the SDK runner if set.                                                                                    |

Each runner is given a **strict env allowlist** (`PATH`, `HOME`, `USER`, `LANG`, `NODE_ENV`, plus
its provider-specific API key). The agent never sees `GH_PAT`, `GH_DEPLOY_KEY`,
`SUPABASE_SERVICE_ROLE_KEY`, etc.

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

| Var                        | Default                         | Purpose                                                                                                                               |
| -------------------------- | ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| `PORT`                     | `8080`                          | HTTP port.                                                                                                                            |
| `HOST`                     | `0.0.0.0`                       | Bind host.                                                                                                                            |
| `BASE_BRANCH`              | `dev`                           | Branch to fetch the thread workspace from.                                                                                            |
| `WORKSPACE_REPO`           | `/home/agent/workspace/repo`    | Persistent thread workspace — seeded from `/home/agent/repo-template` on cold boot, then `git fetch`ed each subsequent boot.          |
| `OUTPUTS_DIR`              | `/home/agent/workspace/outputs` | Where the agent writes publishable files (markdown, PDF, etc.); scanned + uploaded after the agent run exits.                         |
| `REMOTE_DEV_THREAD_ID`     | unset                           | **Required.** The UUID created by `/u/admin/remote-dev` for the conversation. The container is pinned to one thread for its lifetime. |
| `THREAD_LOG_RELATIVE_PATH` | `tmp/convos/thread.log`         | Every prompt/event is appended as JSONL here inside the thread workspace. `tmp/` is gitignored.                                       |
| `DEFAULT_STORAGE_PROVIDER` | `local`                         | One of `local` / `s3` / `r2` / `gcs` / `drive`.                                                                                       |
| `GH_DEPLOY_KEY_PATH`       | `/home/agent/.ssh/id_ed25519`   | Where `GH_DEPLOY_KEY` is materialised at boot.                                                                                        |
| `GIT_AUTHOR_NAME`          | `DD Agent`                      | Commit author.                                                                                                                        |
| `GIT_AUTHOR_EMAIL`         | `agent@dancingdragons.dev`      | Commit email.                                                                                                                         |

### Storage providers — set the block matching your `DEFAULT_STORAGE_PROVIDER`

#### `local` (dev / smoke testing only)

| Var                             | Purpose                                                                  |
| ------------------------------- | ------------------------------------------------------------------------ |
| `LOCAL_STORAGE_ROOT`            | Directory to copy files into. Default `/home/agent/workspace/published`. |
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

| Method | Path                    | Auth                              | Notes                                                                                                                                                    |
| ------ | ----------------------- | --------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `GET`  | `/healthz`              | none                              | Liveness — returns `{ ok, startedAt, inFlightCount, totalTracked }`.                                                                                     |
| `GET`  | `/tasks`                | `X-Server-Auth`                   | Snapshot of every task in memory (used by Vercel to merge with NeonDB on first load).                                                                    |
| `POST` | `/tasks`                | `X-Server-Auth`                   | Body `{ taskId, prompt, userId?, threadId?, provider? }`. Queues the run into the thread workspace. `userId` enables per-user Supabase Realtime fan-out. |
| `GET`  | `/stream/:taskId`       | `X-Server-Auth` **or** `?token=…` | Server-Sent Events. `Last-Event-ID` resumes.                                                                                                             |
| `POST` | `/tasks/:taskId/cancel` | `X-Server-Auth`                   | SIGTERMs the child, emits `done` with `cancelled`.                                                                                                       |

The HMAC token format used by `?token=` is documented in [`src/token.ts`](src/token.ts) — both
sides share `REMOTE_DEV_TOKEN_SECRET`.

## Thread/task lifecycle

For each new thread, the container/workspace:

1. `git fetch origin <BASE_BRANCH>` against the warm base repo.
2. If `agent/thread/<threadId>` already exists remotely, check it out; otherwise create it from
   `origin/<BASE_BRANCH>` and hard-reset to that base. This is what keeps a restarted thread
   container resumable while still making a brand-new thread start from fresh `origin/dev`.
3. `pnpm install --frozen-lockfile` (fast because the image bakes a warm repo and pnpm store).
4. Start listening after the thread workspace is ready in `thread` mode.

For each `POST /tasks` in that thread:

1. Append prompt/event metadata to `tmp/convos/thread.log` as JSONL.
2. `mkdir -p $OUTPUTS_DIR/<taskId>` so the agent has a place to write.
3. Run the selected provider (`claude-sdk` by default, or CLI/OpenAI override).
4. `git add -A && git commit && git push --set-upstream origin agent/thread/<threadId>`.
5. `gh pr view` the thread branch and reuse an existing PR, or create one.
6. **Walk `$OUTPUTS_DIR/<taskId>/`** — every regular file (one level deep) is uploaded via the
   configured storage adapter, emitting one `artifact` event per file with the resulting URL.
7. Emit terminal `done` event.

Tasks are GC'd from memory one hour after completion. The container owns its single workspace for
the lifetime of the pod — sleep/wake of the surrounding K8s Deployment preserves the PVC, so the
workspace remounts intact on the next dispatch.

## Smoke test

After the container is running:

```bash
# direct from the host (uses X-Server-Auth)
curl -fsS http://localhost:8080/healthz   # → {"ok":true}

curl -X POST http://localhost:8080/tasks \
  -H "X-Server-Auth: $SERVER_AUTH_SECRET" \
  -H 'Content-Type: application/json' \
  -d '{"prompt":"echo hi from the agent"}'

# tail the stream
curl -N http://localhost:8080/stream/<taskId> \
  -H "X-Server-Auth: $SERVER_AUTH_SECRET"
```

## Deploying

The production shape is a **per-thread K8s pod** managed by the v4 manifests in
[`remote/k8s/`](../k8s/) — one container pinned to one chat / threadId from `/u/admin/remote-dev`.
The orchestrator
([`src/lib/server/remote-dev/container-registry.ts`](../../src/lib/server/remote-dev/container-registry.ts))
instantiates the per-thread Deployment + Service + PVC + Ingress on first dispatch and tears them
down when the thread is ended.

For local dev, run the container directly with `REMOTE_DEV_THREAD_ID` set to a test UUID — the
server refuses to start without one.

The ECR refresh workflow lives at
[`../../.github/workflows/remote-dev-server-ecr.yml`](../../.github/workflows/remote-dev-server-ecr.yml).
It rebuilds from `dev` at 4am America/New_York every three days, pushing both `latest` and the
commit SHA. Configure `AWS_ROLE_TO_ASSUME`, `AWS_REGION`, `REMOTE_DEV_ECR_REPOSITORY`, and
`DD_REPO_DEPLOY_KEY` in GitHub Secrets/Variables.
