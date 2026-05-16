#!/usr/bin/env bash
# entrypoint.sh - dd-dev-server container lifecycle.
#
# The container refreshes the mounted workspace before starting the server.
# Keeping this synchronous avoids a boot-time race where git fetch/switch and
# the first server-side task preparation both try to take .git/index.lock.
set -euo pipefail

REPO_DIR="${WORKSPACE_REPO:-/home/node/workspace/repo}"
TEMPLATE_DIR="${REPO_TEMPLATE_DIR:-/home/node/repo-template}"
REPO_URL="${DD_REPO_URL:-}"
BASE_BRANCH="${BASE_BRANCH:-${DD_REPO_REF:-dev}}"
DEPLOY_KEY_PATH="${GH_DEPLOY_KEY_PATH:-/home/node/.ssh/id_ed25519}"
SSH_DIR="$(dirname "$DEPLOY_KEY_PATH")"
THREAD_ID="${REMOTE_DEV_THREAD_ID:-${THREAD_ID:-}}"

if [[ -z "$REPO_URL" ]]; then
  echo "DD_REPO_URL is required; build and run the worker with an explicit git repo URL" >&2
  exit 64
fi

echo "==> dd-dev-server entrypoint starting at $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "    workspace=$REPO_DIR source=$REPO_URL branch=$BASE_BRANCH thread=${THREAD_ID:-<multi-thread>}"

export CI="${CI:-true}"
export COREPACK_ENABLE_DOWNLOAD_PROMPT="${COREPACK_ENABLE_DOWNLOAD_PROMPT:-0}"
export PNPM_STORE_DIR="${PNPM_STORE_DIR:-$REPO_DIR/.pnpm-store}"
export npm_config_store_dir="${npm_config_store_dir:-$PNPM_STORE_DIR}"

# Seed the persistent workspace from the baked template on cold boot
# (PVC empty). Warm boots — where $REPO_DIR/.git already exists from
# a previous mount of the same PVC — skip this entirely and rely on
# the `git fetch` below to catch up on delta.
if [[ ! -d "$REPO_DIR/.git" && -d "$TEMPLATE_DIR/.git" ]]; then
  echo "==> Cold-boot seed: copying $TEMPLATE_DIR -> $REPO_DIR"
  mkdir -p "$REPO_DIR"
  cp -a "$TEMPLATE_DIR/." "$REPO_DIR/"
fi

if [[ -d "$REPO_DIR/.git" ]]; then
  find "$REPO_DIR/.git" -maxdepth 1 -type f -name index.lock -delete 2>/dev/null || true
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

if [[ -d "$REPO_DIR/.git" ]]; then
  echo "==> git fetch + switch starting"
  cd "$REPO_DIR"
  git remote set-url origin "$REPO_URL" 2>&1 || echo "git remote set-url failed (non-fatal)"
  git fetch --quiet origin "$BASE_BRANCH" --depth=50 2>&1 || echo "git fetch failed (non-fatal)"
  git switch --discard-changes --detach "origin/$BASE_BRANCH" 2>&1 || echo "git switch failed (non-fatal)"
  git clean -fdx \
    --exclude=node_modules \
    --exclude=.pnpm-store \
    --exclude=.next \
    --exclude=.turbo 2>&1 || true

  if [[ -f package.json ]]; then
    PNPM_VERSION="$(pnpm --version 2>/dev/null || true)"
    PNPM_STORE_PATH="$(pnpm store path --store-dir "$PNPM_STORE_DIR" 2>/dev/null || true)"
    echo "==> pnpm install starting (version=${PNPM_VERSION:-unknown} store=${PNPM_STORE_PATH:-unknown})"
    if ! pnpm install --store-dir "$PNPM_STORE_DIR" --frozen-lockfile --offline 2>&1; then
      echo "offline frozen pnpm install failed; retrying prefer-offline"
      if ! pnpm install --store-dir "$PNPM_STORE_DIR" --frozen-lockfile --prefer-offline 2>&1; then
        echo "frozen prefer-offline pnpm install failed; retrying fallback"
        pnpm install --store-dir "$PNPM_STORE_DIR" --prefer-offline 2>&1 || echo "fallback pnpm install failed (non-fatal)"
      fi
    fi
  else
    echo "==> no root package.json in workspace; skipping pnpm install"
  fi
  echo "==> git refresh complete at $(date -u +%Y-%m-%dT%H:%M:%SZ)"
fi

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

echo "==> Starting server"
exec node /srv/dev-server/dist/server.js
