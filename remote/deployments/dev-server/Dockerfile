# syntax=docker/dockerfile:1.7
#
# dd-dev-server — long-lived Node.js + TypeScript HTTP server that
# runs Claude/OpenAI coding agents against a baked, warm checkout of the
# configured Git repo and streams events out (SSE / webhook / Supabase Broadcast).
#
# Primary deployment shape: per-thread K8s pod — one container pinned
# to a chat / threadId, instantiated from the v4 manifests in
# `remote/k8s/`. The server refuses to start without REMOTE_DEV_THREAD_ID.
# Also runs as a standalone container for local dev (set the env var
# manually).
#
# Build (BuildKit required for --secret):
#   DOCKER_BUILDKIT=1 docker build \
#     --build-arg DD_REPO_URL=git@github.com:org/repo.git \
#     --build-arg DD_REPO_REF=dev \
#     --secret id=github_deploy_key,src=/path/to/dd_deploy \
#     -t dd-dev-server:latest \
#     remote/deployments/dev-server
#
# Run:
#   docker run --rm -p 8080:8080 \
#     -e ANTHROPIC_API_KEY=... \
#     -e GH_PAT=... \
#     -e GH_DEPLOY_KEY="$(cat /path/to/dd_deploy)" \
#     -e EVENT_INGEST_URL=https://example.internal/api/agent/events \
#     -e EVENT_INGEST_SECRET=... \
#     -e SERVER_AUTH_SECRET=... \
#     dd-dev-server:latest

ARG NODE_VERSION=22

# ============================================================
# Stage 1 — Build the TypeScript server
# ============================================================
FROM node:${NODE_VERSION}-bookworm-slim AS builder
WORKDIR /build

RUN corepack enable \
 && corepack prepare pnpm@9.15.4 --activate \
 && pnpm --version

# Install full deps (including dev) so we can compile TS.
COPY package.json pnpm-lock.yaml* tsconfig.json ./
RUN if [ -f pnpm-lock.yaml ]; then pnpm install --frozen-lockfile; else pnpm install --no-frozen-lockfile; fi

# Compile, then prune dev deps so node_modules is runtime-only.
COPY src ./src
RUN pnpm run build \
 && pnpm prune --prod

# ============================================================
# Stage 2 — Runtime image
# ============================================================
FROM node:${NODE_VERSION}-bookworm-slim AS runtime

# --- OS packages ---
# git: clone/pull/push the working repo
# gh:  open PRs from the agent
# openssh-client: deploy key + ssh-keyscan
# tini: reap zombie claude/git/pnpm children spawned by the server
# util-linux: provides `script`, used to give the browser terminal a PTY
# curl, jq: misc agent / healthcheck use
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl gnupg git openssh-client jq tini util-linux \
 && install -m 0755 -d /etc/apt/keyrings \
 && curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
    | gpg --dearmor -o /etc/apt/keyrings/github-cli.gpg \
 && echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/github-cli.gpg] https://cli.github.com/packages stable main" \
    > /etc/apt/sources.list.d/github-cli.list \
 && apt-get update \
 && apt-get install -y --no-install-recommends gh \
 && rm -rf /var/lib/apt/lists/*

# --- Global node tooling: pnpm + agent CLIs ---
# Both CLIs are installed unconditionally so the runtime can flip between
# providers via AGENT_PROVIDER without rebuilding the image. Drop one if
# you only ever use one provider and want a smaller image.
#   * `claude`  → @anthropic-ai/claude-code  (drives `claude-cli` runner)
#   * `codex`   → @openai/codex              (drives `openai-codex-cli` runner)
# If `@openai/codex` isn't published under that name in the version you
# need, swap the package or drop the codex install — the runner does a
# `which codex` preflight at /agents and reports unavailable cleanly.
RUN corepack enable \
 && corepack prepare pnpm@9.15.4 --activate \
 && pnpm --version \
 && npm i -g @anthropic-ai/claude-code \
 && (npm i -g @openai/codex 2>/dev/null || echo "codex CLI not installed; openai-codex-cli runner will be marked unavailable")

# --- Non-root user + writable dirs ---
# node:22-bookworm-slim already ships with uid/gid 1000 (`node`). The runtime
# image uses that user directly; Kubernetes init containers only chown mounted
# hostPath workspaces before this process starts.
RUN mkdir -p /srv/dev-server /home/node/workspace /home/node/.ssh \
 && chown -R node:node /srv/dev-server /home/node

USER node
ENV HOME=/home/node \
    USER=node

# ============================================================
# Optionally bake a "recent baseline" repo with warm node_modules.
# Repo-scoped container-pool workers can also start from this generic image
# and clone DD_REPO_URL at runtime, so DD_REPO_URL is not required at build time.
# ============================================================
WORKDIR /home/node
ARG DD_REPO_URL
ARG DD_REPO_REF=dev
ARG DD_REPO_CACHE_BUST=manual
RUN --mount=type=secret,id=github_deploy_key,uid=1000,mode=0600 \
    if [ -n "$DD_REPO_URL" ]; then \
      mkdir -p /home/node/.ssh \
      && echo "$DD_REPO_CACHE_BUST" > /tmp/dd-repo-cache-bust \
      && cp /run/secrets/github_deploy_key /home/node/.ssh/id_ed25519 \
      && chmod 700 /home/node/.ssh \
      && chmod 600 /home/node/.ssh/id_ed25519 \
      && touch /home/node/.ssh/known_hosts \
      && GIT_SSH_COMMAND="ssh -F /dev/null -i /home/node/.ssh/id_ed25519 -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=/home/node/.ssh/known_hosts" \
        git clone --depth 50 --branch "$DD_REPO_REF" "$DD_REPO_URL" repo-template \
      && cd repo-template \
      && if [ -f package.json ]; then \
        pnpm install --store-dir /home/node/repo-template/.pnpm-store --frozen-lockfile; \
      else \
        echo "no root package.json in repo-template; skipping pnpm install"; \
      fi \
      && rm -f /home/node/.ssh/id_ed25519; \
    else \
      mkdir -p /home/node/repo-template \
      && echo "DD_REPO_URL not provided; building generic repo-configured worker base"; \
    fi
ENV DD_REPO_REF=$DD_REPO_REF \
    BASE_BRANCH=$DD_REPO_REF

# --- Compiled Node server bundle ---
WORKDIR /srv/dev-server
COPY --from=builder --chown=node:node /build/package.json /build/pnpm-lock.yaml ./
COPY --from=builder --chown=node:node /build/node_modules ./node_modules
COPY --from=builder --chown=node:node /build/dist ./dist

# Container entrypoint — handles git fetch + workspace sync on start, then
# launches the server. See entrypoint.sh for the full flow.
COPY --chown=node:node --chmod=0755 entrypoint.sh ./entrypoint.sh

# --- Runtime config ---
# WORKSPACE_REPO    — warm repo; thread mode uses it as the workspace
# GH_DEPLOY_KEY_PATH — where the server writes GH_DEPLOY_KEY at startup
# EVENT_INGEST_URL  — Vercel route that ingests streamed agent events
# EVENT_INGEST_SECRET — shared secret for the ingest header
# SERVER_AUTH_SECRET — shared secret callers must present on POST /tasks
ENV NODE_ENV=production \
    PORT=8080 \
    WORKSPACE_REPO=/home/node/workspace/repo \
    GH_DEPLOY_KEY_PATH=/home/node/.ssh/id_ed25519 \
    LOG_DIR=/tmp/convos

EXPOSE 8080

# tini as PID 1 so child agent/git processes don't become zombies
# when the server kills them on cancel / shutdown.
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["/srv/dev-server/entrypoint.sh"]

HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
  CMD curl -fsS http://localhost:8080/healthz || exit 1
