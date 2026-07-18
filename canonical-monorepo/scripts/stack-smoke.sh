#!/usr/bin/env bash
# Boot the stack that ./build.sh just produced and prove the cross-repo
# contracts hold at runtime:
#   - the Rust server serves the built Astro marketing site as static fallback
#   - the built HTMX/IndexedDB client assets are mounted at /app-assets
#   - /healthz and /readyz answer
#   - /api/v1/health and /api/v1/info conform to the canonical-interfaces
#     JSON Schema pinned in apps/canonical-interfaces (the typed-IO seam)
#   - unknown /api paths return JSON 404s, not marketing HTML
#
# Requires: ./build.sh already ran (release binary + both dists exist).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SERVER="$ROOT/apps/canonical-web-server.rs/target/release/canonical-web-server"
MARKETING_DIST="$ROOT/apps/canonical-marketing-site.web/dist"
CLIENT_DIST="$ROOT/apps/canonical-web-server.rs/client/dist"
API_SCHEMA="$ROOT/apps/canonical-interfaces/schema/api.schema.json"
PORT="${SMOKE_PORT:-18091}"
BASE="http://127.0.0.1:$PORT"
SMOKE_DB_URL="sqlite://${TMPDIR:-/tmp}/canonical-cloud-smoke-${PPID}-$$.sqlite?mode=rwc"

for artifact in "$SERVER" "$MARKETING_DIST/index.html" "$API_SCHEMA"; do
  if [[ ! -e "$artifact" ]]; then
    echo "error: missing $artifact — run ./build.sh first" >&2
    exit 1
  fi
done

server_pid=""
cleanup() {
  if [[ -n "$server_pid" ]]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
}
trap cleanup EXIT

MIGRATION_DATABASE_URL="$SMOKE_DB_URL" \
  MIGRATION_DATABASE_MAX_CONNECTIONS=1 \
  "$SERVER" migrate

PORT="$PORT" \
  APP_BASE_URL="$BASE" \
  APP_ALLOWED_ORIGINS="$BASE" \
  APP_SESSION_ENCRYPTION_KEY="AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=" \
  COOKIE_SECURE=false \
  DATABASE_URL="$SMOKE_DB_URL" \
  DATABASE_MAX_CONNECTIONS=1 \
  SUPABASE_URL="http://127.0.0.1:54321" \
  SUPABASE_PUBLISHABLE_KEY="sb_publishable_smoke_only" \
  STATIC_DIR="$MARKETING_DIST" \
  APP_ASSET_DIR="$CLIENT_DIST" \
  "$SERVER" serve &
server_pid=$!

for attempt in $(seq 1 30); do
  if curl --fail --silent "$BASE/healthz" >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$server_pid" 2>/dev/null; then
    echo "error: server exited before becoming healthy" >&2
    exit 1
  fi
  if [[ "$attempt" == 30 ]]; then
    echo "error: server never became healthy on $BASE" >&2
    exit 1
  fi
  sleep 1
done

echo "==> /healthz and /readyz"
curl --fail --silent "$BASE/healthz" >/dev/null
curl --fail --silent "$BASE/readyz" >/dev/null

echo "==> marketing site served as static fallback"
home="$(curl --fail --silent "$BASE/")"
grep --quiet --ignore-case "<html" <<<"$home"

echo "==> built client asset served under /app-assets"
asset="$(cd "$CLIENT_DIST" && find . -type f \( -name '*.js' -o -name '*.css' \) | head -1)"
asset="${asset#./}"
if [[ -z "$asset" ]]; then
  echo "error: no built asset found in $CLIENT_DIST" >&2
  exit 1
fi
curl --fail --silent "$BASE/app-assets/$asset" >/dev/null

echo "==> /api/v1/health conforms to interfaces HealthStatus"
curl --fail --silent "$BASE/api/v1/health" |
  node "$ROOT/scripts/validate-against-schema.mjs" "$API_SCHEMA" HealthStatus -

echo "==> /api/v1/info conforms to interfaces ServiceInfo"
curl --fail --silent "$BASE/api/v1/info" |
  node "$ROOT/scripts/validate-against-schema.mjs" "$API_SCHEMA" ServiceInfo -

echo "==> unknown /api path is a JSON 404, not marketing HTML"
not_found_type="$(curl --silent --output /dev/null --write-out '%{http_code} %{content_type}' "$BASE/api/v1/definitely-not-a-route")"
case "$not_found_type" in
  "404 application/json"*) ;;
  *)
    echo "error: expected '404 application/json', got '$not_found_type'" >&2
    exit 1
    ;;
esac

echo "stack smoke passed"
