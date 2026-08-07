#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${K8S_SUBMODULE_TOKEN:-}" ]]; then
  echo "K8S_SUBMODULE_TOKEN is required" >&2
  exit 2
fi
if (($# == 0)); then
  echo "usage: init-private-submodules.sh <path> [<path> ...]" >&2
  exit 2
fi

# actions/checkout must use persist-credentials: false before this helper runs.
# Otherwise its repository-scoped GITHUB_TOKEN extraheader can override the
# broader private-submodule credential embedded by the URL rewrite below.
token_url="https://x-access-token:${K8S_SUBMODULE_TOKEN}@github.com/"
cleanup() {
  git config --global --unset-all url."${token_url}".insteadOf >/dev/null 2>&1 || true
}
trap cleanup EXIT

# The repository intentionally contains both SSH and HTTPS submodule URLs.
# Rewrite both forms to the same short-lived/private-read credential so mixed
# organization checkouts do not silently fall back to the outer GITHUB_TOKEN.
git config --global --add url."${token_url}".insteadOf 'git@github.com:'
git config --global --add url."${token_url}".insteadOf 'https://github.com/'

export GIT_TERMINAL_PROMPT=0
git submodule sync -- "$@"
git submodule update --init --depth 1 -- "$@"
