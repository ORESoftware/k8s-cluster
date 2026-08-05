#!/usr/bin/env bash
set -Eeuo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

bash scripts/validate-zed-submodules.sh
command -v zed >/dev/null 2>&1 || { echo 'zed is required' >&2; exit 127; }
zed install --git-submodules "$@"
bash scripts/validate-zed-submodules.sh
