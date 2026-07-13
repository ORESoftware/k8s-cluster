#!/usr/bin/env bash
# Build the canonical.cloud Astro marketing site, the authenticated HTMX /
# IndexedDB client, and the Rust sMASH application server. Each app keeps its
# output in its own submodule; STATIC_DIR points the server at the Astro dist.
#
# Runs against the submodule checkouts under apps/. Ensure they are initialized:
#   git submodule update --init --recursive
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MARKETING_SITE="$ROOT/apps/canonical-marketing-site.web"
WEB_SERVER="$ROOT/apps/canonical-web-server.rs"
APP_CLIENT="$WEB_SERVER/client"

if [[ ! -d "$MARKETING_SITE/src" || ! -d "$WEB_SERVER/src" || ! -d "$APP_CLIENT/src" ]]; then
  echo "error: submodules not initialized. Run: git submodule update --init --recursive" >&2
  exit 1
fi

echo "==> Building Astro marketing site"
(cd "$MARKETING_SITE" && npm ci && npm run build)

echo "==> Verifying and building HTMX / IndexedDB application client"
(cd "$APP_CLIENT" && npm ci && npm run typecheck && npm test && npm run build)

echo "==> Building Rust sMASH web server"
(cd "$WEB_SERVER" && cargo build --locked --release)

echo "==> Done. Configure an ignored .env.local from .env.example, then run:"
echo "    set -a; source .env.local; set +a"
echo "    ./apps/canonical-web-server.rs/target/release/canonical-web-server migrate"
echo "    unset MIGRATION_DATABASE_URL MIGRATION_DATABASE_MAX_CONNECTIONS"
echo "    ./apps/canonical-web-server.rs/target/release/canonical-web-server serve"
