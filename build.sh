#!/usr/bin/env bash
# Build the canonical.cloud Astro marketing site, authenticated application
# client, and Rust web server without copying generated trees between repos.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MARKETING_SITE="$ROOT/canonical-marketing-site.web"
WEB_SERVER="$ROOT/canonical-web-server.rs"

echo "==> Building Astro marketing site"
(cd "$MARKETING_SITE" && npm ci && npm run build)

echo "==> Verifying and building HTMX / IndexedDB client"
(cd "$WEB_SERVER/client" && npm ci && npm run typecheck && npm test && npm run build)

echo "==> Building Rust web server"
(cd "$WEB_SERVER" && cargo build --locked --release)

echo "==> Done. Configure canonical-web-server.rs/.env.local, then run:"
echo "    (cd canonical-web-server.rs && STATIC_DIR=../canonical-marketing-site.web/dist ./target/release/canonical-web-server)"
