#!/usr/bin/env bash
# Local dev runner for dd-dart-server.
#
# Runs the server in JIT mode with the VM service enabled so the
# in-process HotReloader can call reloadSources() across every isolate
# whenever a .dart file under lib/ or bin/ changes.
#
# Connect from any browser:
#   http://localhost:8089/dart/pages
#   http://localhost:8089/dart/pages/wss
#
# Watch hot reload progress on the same process's stdout, or hit:
#   curl http://localhost:8089/dart/admin/hot-reload-status | jq .
#
# Trigger a manual reload:
#   curl -XPOST http://localhost:8089/dart/admin/reload
#
# Edit, save, watch the pipeline rebuild WITHOUT dropping any open
# WebSocket. The next render frame uses the new code.

set -euo pipefail

cd "$(dirname "$0")/.."

export HTTP_HOST="${HTTP_HOST:-127.0.0.1}"
export HTTP_PORT="${HTTP_PORT:-8089}"
export STATIC_DIR="${STATIC_DIR:-$PWD/flutter_app/build/web}"
export HOT_RELOAD=true
export HOT_RELOAD_PATHS="${HOT_RELOAD_PATHS:-lib,bin}"

dart pub get >/dev/null

# Generate jaspr_options.dart on first run. Subsequent runs skip if the
# generated file is fresh.
if [ ! -f lib/jaspr_options.dart ] || [ pubspec.yaml -nt lib/jaspr_options.dart ]; then
  dart run build_runner build --delete-conflicting-outputs
fi

# Try to keep a Flutter web build around (optional; the SSR demo at
# /dart/pages/wss works without it). If you have flutter on PATH and want
# the SPA at /dart/app, run `flutter build web` separately.
if [ ! -f "$STATIC_DIR/index.html" ]; then
  echo "[dev.sh] $STATIC_DIR is empty — /dart/app will return 404 until you run"
  echo "          (cd flutter_app && flutter build web --pwa-strategy=offline-first --base-href=/dart/app/)"
fi

# `--enable-vm-service` exposes the VM service WebSocket the HotReloader
# connects to in-process via Service.getInfo().
# `--disable-service-auth-codes` removes the random auth-code path on the
# service URL so manual `curl` against it (rare, for debugging) works.
exec dart run \
  --enable-vm-service=8181 \
  --disable-service-auth-codes \
  --no-serve-devtools \
  bin/server.dart "$@"
