#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIR"

if ! command -v mise >/dev/null 2>&1; then
  cat >&2 <<'EOF'
ClipTown bootstrap requires mise.
Install mise through its official package or installer, then rerun:
  bash scripts/bootstrap.sh

This script intentionally does not curl an installer or request production credentials.
EOF
  exit 2
fi

mise install

git submodule sync --recursive
git submodule update --init --recursive

bash scripts/preflight.sh

cat <<'EOF'
ClipTown development prerequisites are ready.
Run the low-risk cross-repository checks with:
  bash scripts/validate-workspace.sh
EOF
