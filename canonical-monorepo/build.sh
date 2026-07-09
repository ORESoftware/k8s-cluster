#!/usr/bin/env bash
# Build the canonical.cloud Astro frontend and publish it into the
# canonical-backend.rs static dir, then build the Rust backend.
#
# Runs against the submodule checkouts under apps/. Ensure they are initialized:
#   git submodule update --init --recursive
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FRONTEND="$ROOT/apps/canonical-frontend"
BACKEND="$ROOT/apps/canonical-backend.rs"

if [[ ! -d "$FRONTEND/src" || ! -d "$BACKEND/src" ]]; then
  echo "error: submodules not initialized. Run: git submodule update --init --recursive" >&2
  exit 1
fi

echo "==> Building Astro frontend"
(cd "$FRONTEND" && npm ci && npm run build)

echo "==> Publishing dist/ -> backend static/ (via git rm/checkout-safe copy)"
rm -rf "$BACKEND/static"
cp -R "$FRONTEND/dist" "$BACKEND/static"

echo "==> Building Rust backend"
(cd "$BACKEND" && cargo build --release)

echo "==> Done. Run with:"
echo "    (cd apps/canonical-backend.rs && PORT=8081 ./target/release/canonical-backend)"
