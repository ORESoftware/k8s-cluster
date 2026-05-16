#!/usr/bin/env bash
# entrypoint.sh - dd-dev-server container lifecycle.
#
# The container starts serving quickly, while a background workspace sync
# warms the baked checkout. server.ts waits for GIT_READY_PID before the
# first task touches the worktree, so requests do not branch from stale code.
set -euo pipefail

REPO_DIR="${WORKSPACE_REPO:-/home/agent/workspace/repo}"
TEMPLATE_DIR="${REPO_TEMPLATE_DIR:-/home/agent/repo-template}"
BASE_BRANCH="${BASE_BRANCH:-dev}"
DEPLOY_KEY_PATH="${GH_DEPLOY_KEY_PATH:-/home/agent/.ssh/id_ed25519}"
SSH_DIR="$(dirname "$DEPLOY_KEY_PATH")"
THREAD_ID="${REMOTE_DEV_THREAD_ID:-${THREAD_ID:-}}"

echo "==> dd-dev-server entrypoint starting at $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "    repo=$REPO_DIR branch=$BASE_BRANCH thread=${THREAD_ID:-<multi-thread>}"

# Seed the persistent workspace from the baked template on cold boot
# (PVC empty). Warm boots — where $REPO_DIR/.git already exists from
# a previous mount of the same PVC — skip this entirely and rely on
# the background `git fetch` below to catch up on delta.
if [[ ! -d "$REPO_DIR/.git" && -d "$TEMPLATE_DIR/.git" ]]; then
  echo "==> Cold-boot seed: copying $TEMPLATE_DIR -> $REPO_DIR"
  mkdir -p "$REPO_DIR"
  cp -a "$TEMPLATE_DIR/." "$REPO_DIR/"
fi

# Local-only: if the previous container crashed mid-git-op, stale
# *.lock files in $REPO_DIR/.git block every subsequent git command.
# Production never sees this because crashes are rare and EBS PVCs
# are reprovisioned on End Thread; in minikube the same PVC sticks
# around across CrashLoopBackOff restarts. Clear stale locks before
# anything else touches the worktree.
if [[ -d "$REPO_DIR/.git" ]]; then
  while IFS= read -r stale_lock; do
    echo "==> Removed stale git lock: $stale_lock"
  done < <(find "$REPO_DIR/.git" -maxdepth 2 -name '*.lock' -type f -print -delete 2>/dev/null) || true
fi

mkdir -p "$SSH_DIR"

if [[ -n "${GH_DEPLOY_KEY:-}" ]]; then
  echo "==> Writing deploy key to $DEPLOY_KEY_PATH"
  printf '%s\n' "$GH_DEPLOY_KEY" > "$DEPLOY_KEY_PATH"
  chmod 600 "$DEPLOY_KEY_PATH"
  ssh-keyscan github.com >> "$SSH_DIR/known_hosts" 2>/dev/null || true
fi

cat > "$SSH_DIR/config" <<EOF
Host github.com
  HostName github.com
  User git
  IdentitiesOnly yes
  StrictHostKeyChecking yes
  UserKnownHostsFile $SSH_DIR/known_hosts
EOF
chmod 600 "$SSH_DIR/config"

# StrictHostKeyChecking=yes uses the known_hosts populated above by
# ssh-keyscan. NEVER weaken to `no` or point at /dev/null — that opens
# `git push` to MitM substitution of github.com.
export GIT_SSH_COMMAND="ssh -i $DEPLOY_KEY_PATH -o StrictHostKeyChecking=yes -o UserKnownHostsFile=$SSH_DIR/known_hosts"

(
  echo "==> [background] git fetch + switch starting"
  cd "$REPO_DIR"
  git fetch --quiet origin "$BASE_BRANCH" --depth=50 2>&1 || echo "[background] git fetch failed (non-fatal)"
  git switch --discard-changes --detach "origin/$BASE_BRANCH" 2>&1 || echo "[background] git switch failed (non-fatal)"
  git clean -fdx \
    --exclude=node_modules \
    --exclude=.pnpm-store \
    --exclude=.next \
    --exclude=.turbo 2>&1 || true

  echo "==> [background] pnpm install starting"
  pnpm install --frozen-lockfile --prefer-offline 2>&1 \
    || pnpm install --prefer-offline 2>&1 \
    || echo "[background] pnpm install failed (non-fatal)"
  echo "==> [background] git refresh complete at $(date -u +%Y-%m-%dT%H:%M:%SZ)"
) &
GIT_READY_PID=$!
export GIT_READY_PID
echo "==> git refresh running in background (PID $GIT_READY_PID)"

LOG_BASE="${CONVO_LOG_DIR:-/tmp/convos}"
if [[ -n "${THREAD_ID:-}" ]]; then
  LOG_DIR="$LOG_BASE/$THREAD_ID"
else
  LOG_DIR="$LOG_BASE"
fi
mkdir -p "$LOG_DIR"
export LOG_DIR
export THREAD_ID="${THREAD_ID:-}"
export REMOTE_DEV_THREAD_ID="${REMOTE_DEV_THREAD_ID:-$THREAD_ID}"
export IDLE_TIMEOUT_MS="${IDLE_TIMEOUT_MS:-1800000}"
echo "==> Logs at $LOG_DIR/thread.log"

# Local-only: wait for the background git fetch + pnpm install to finish
# before starting the server. In prod the server starts concurrently and
# runTask blocks on GIT_READY_PID, but main()'s pre-warm bootSession races
# with the still-running background `git switch` and dies on
# `.git/index.lock: File exists`. Synchronous boot trades cold-start
# latency for correctness here.
echo "==> Waiting for background git refresh (PID $GIT_READY_PID) to complete"
wait "$GIT_READY_PID" || true

echo "==> Starting server"
exec node /srv/dev-server/dist/server.js
