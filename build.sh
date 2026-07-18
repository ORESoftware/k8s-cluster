#!/usr/bin/env bash
# Build the canonical.cloud Astro marketing site, authenticated application
# client, and every Rust workspace binary without copying generated trees
# between repositories. Deployment releases are pinned and verified by
# canonical-monorepo; this is the sibling-checkout development equivalent.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MARKETING_SITE="$ROOT/canonical-marketing-site.web"
WEB_SERVER="$ROOT/canonical-web-server.rs"

echo "==> Building Astro marketing site"
(cd "$MARKETING_SITE" && npm ci && npm run build)

echo "==> Verifying and building HTMX / IndexedDB client"
(cd "$WEB_SERVER/client" && npm ci && npm run typecheck && npm test && npm run build)

echo "==> Building Rust workspace binaries (web server + session revoker)"
(cd "$WEB_SERVER" && cargo build --locked --release --workspace --bins)

echo "==> Done. Derive isolated ignored environments from canonical-web-server.rs/.env.example, then run:"
echo "    # one-shot migration environment"
echo "    (cd canonical-web-server.rs && ./target/release/canonical-web-server migrate)"
echo "    # customer web environment (no migration or revoker database URL)"
echo "    unset MIGRATION_DATABASE_URL MIGRATION_DATABASE_MAX_CONNECTIONS SESSION_REVOCATION_DATABASE_URL SESSION_REVOCATION_DATABASE_MAX_CONNECTIONS"
echo "    (cd canonical-web-server.rs && STATIC_DIR=../canonical-marketing-site.web/dist ./target/release/canonical-web-server serve)"
echo "    # separate no-ingress revoker environment"
echo "    (cd canonical-web-server.rs && ./target/release/canonical-session-revoker run)"
