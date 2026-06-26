#!/usr/bin/env bash
# Build the canonical.cloud Astro frontend and publish it into the
# canonical-backend.rs static dir, then build the Rust backend.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FRONTEND="$ROOT/canonical-frontend"
BACKEND="$ROOT/canonical-backend.rs"

echo "==> Building Astro frontend"
(cd "$FRONTEND" && npm run build)

echo "==> Publishing dist/ -> backend static/"
rm -rf "$BACKEND/static"
cp -R "$FRONTEND/dist" "$BACKEND/static"

echo "==> Building Rust backend"
(cd "$BACKEND" && cargo build --release)

echo "==> Done. Run with: (cd canonical-backend.rs && PORT=8081 ./target/release/canonical-backend)"
