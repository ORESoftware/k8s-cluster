# syntax=docker/dockerfile:1.7
#
# dd-dev-server-local — minikube-friendly Node.js + TypeScript HTTP server that
# runs Claude/OpenAI coding agents against a baked, warm checkout of a
# configured Git repo and streams events out (SSE / webhook / Supabase Broadcast).
#
# Primary deployment shape: one local minikube pod pinned to a chat /
# threadId from `k8s/minikube-dev-server.yaml`. Production EC2 Kubernetes
# manifests live in `remote/k8s/`; keep this folder laptop-only.
#
# Build (BuildKit required for --secret):
#   DOCKER_BUILDKIT=1 docker build \
#     --build-arg DD_REPO_URL=git@github.com:org/repo.git \
#     --build-arg DD_REPO_REF=dev \
#     --secret id=github_deploy_key,src=/path/to/dd_deploy \
#     -t dd-dev-server-local:latest \
#     remote/dev-server-local
#
# Run:
#   docker run --rm -p 8080:8080 \
#     -e ANTHROPIC_API_KEY=... \
#     -e GH_PAT=... \
#     -e GH_DEPLOY_KEY="$(cat /path/to/dd_deploy)" \
#     -e EVENT_INGEST_URL=https://dd-next-1.vercel.app/api/agent/events \
#     -e EVENT_INGEST_SECRET=... \
#     -e SERVER_AUTH_SECRET=... \
#     dd-dev-server-local:latest

ARG NODE_VERSION=22

# ============================================================
# Stage 1 — Build the TypeScript server
# ============================================================
FROM node:${NODE_VERSION}-bookworm-slim AS builder
WORKDIR /build

RUN corepack enable && corepack prepare pnpm@9 --activate

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
# curl, jq: misc agent / healthcheck use
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl gnupg git openssh-client jq tini \
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
 && corepack prepare pnpm@9 --activate \
 && npm i -g @anthropic-ai/claude-code \
 && (npm i -g @openai/codex 2>/dev/null || echo "codex CLI not installed; openai-codex-cli runner will be marked unavailable")

# --- Non-root user + writable dirs ---
# node:22-bookworm-slim ships a `node` user at UID 1000; remove it so
# our `agent` user can claim that UID (matches K8s securityContext.runAsUser).
RUN (userdel -r node 2>/dev/null || true) \
 && useradd -m -u 1000 -s /bin/bash agent \
 && mkdir -p /srv/dev-server /home/agent/workspace \
 && chown -R agent:agent /srv/dev-server /home/agent/workspace

USER agent

# ============================================================
# Bake the "recent baseline" — configured repo with warm node_modules.
# Baked at /home/agent/repo-template (NOT under workspace/) so the
# v4 PVC mount at /home/agent/workspace doesn't mask it. entrypoint.sh
# seeds /home/agent/workspace/repo from this template on first boot
# (cold start with empty PVC); subsequent reboots reuse the persisted
# workspace and just `git fetch` the recent delta.
# ============================================================
WORKDIR /home/agent
ARG DD_REPO_URL
ARG DD_REPO_REF=dev
# node:22-bookworm-slim omits build-essential, but dd-next-1's deps include
# native modules (bufferutil, utf-8-validate) that need python3/make/g++ via
# node-gyp at install time. Install briefly as root, run pnpm install as the
# agent user, then strip build tools so they don't bloat the runtime image.
USER root
RUN apt-get update \
 && apt-get install -y --no-install-recommends python3 make g++ \
 && rm -rf /var/lib/apt/lists/*
USER agent
RUN --mount=type=secret,id=github_deploy_key,uid=1000,mode=0600 \
    test -n "$DD_REPO_URL" \
 && mkdir -p /home/agent/.ssh \
 && cp /run/secrets/github_deploy_key /home/agent/.ssh/id_ed25519 \
 && chmod 600 /home/agent/.ssh/id_ed25519 \
 && ssh-keyscan github.com >> /home/agent/.ssh/known_hosts 2>/dev/null \
 && git clone --depth 50 --branch "$DD_REPO_REF" "$DD_REPO_URL" repo-template \
 && cd repo-template \
 && pnpm install --frozen-lockfile \
 && rm -f /home/agent/.ssh/id_ed25519
USER root
RUN apt-get purge -y python3 make g++ \
 && apt-get autoremove -y \
 && rm -rf /var/lib/apt/lists/*
USER agent

# --- Compiled Node server bundle ---
WORKDIR /srv/dev-server
COPY --from=builder --chown=agent:agent /build/package.json /build/pnpm-lock.yaml ./
COPY --from=builder --chown=agent:agent /build/node_modules ./node_modules
COPY --from=builder --chown=agent:agent /build/dist ./dist

# Container entrypoint — handles git fetch + workspace sync on start, then
# launches the server. See entrypoint.sh for the full flow.
COPY --chown=agent:agent entrypoint.sh ./entrypoint.sh
RUN chmod +x entrypoint.sh

# --- Runtime config ---
# WORKSPACE_REPO    — warm repo; thread mode uses it as the workspace
# GH_DEPLOY_KEY_PATH — where the server writes GH_DEPLOY_KEY at startup
# EVENT_INGEST_URL  — Vercel route that ingests streamed agent events
# EVENT_INGEST_SECRET — shared secret for the ingest header
# SERVER_AUTH_SECRET — shared secret callers must present on POST /tasks
ENV NODE_ENV=production \
    PORT=8080 \
    WORKSPACE_REPO=/home/agent/workspace/repo \
    GH_DEPLOY_KEY_PATH=/home/agent/.ssh/id_ed25519 \
    LOG_DIR=/tmp/convos

EXPOSE 8080

# tini as PID 1 so child agent/git processes don't become zombies
# when the server kills them on cancel / shutdown.
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["/srv/dev-server/entrypoint.sh"]

HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
  CMD curl -fsS http://localhost:8080/healthz || exit 1
