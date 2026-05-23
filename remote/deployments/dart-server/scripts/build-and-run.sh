#!/usr/bin/env bash
# In-cluster build + run script for dd-dart-server.
#
# This mirrors the akka-ws-server / billing-server-rs pattern: the
# Kubernetes pod mounts the repo from the EC2 host and runs this script
# at start so the binary stays in sync with the checked-out source. The
# Dockerfile path is for local-laptop builds; production rolls through
# this script.
#
# Required env vars (defaulted):
#   FLUTTER_HOME   /opt/flutter
#   DART_HOME      ${FLUTTER_HOME}/bin/cache/dart-sdk
#   STATIC_DIR     ${PWD}/public
#   HTTP_HOST      0.0.0.0
#   HTTP_PORT      8089

set -euo pipefail

FLUTTER_HOME="${FLUTTER_HOME:-/opt/flutter}"
DART_HOME="${DART_HOME:-${FLUTTER_HOME}/bin/cache/dart-sdk}"
STATIC_DIR="${STATIC_DIR:-${PWD}/public}"

export PATH="${FLUTTER_HOME}/bin:${DART_HOME}/bin:${PATH}"

echo "[dd-dart-server] dart $(dart --version 2>&1)"
echo "[dd-dart-server] flutter $(flutter --version 2>&1 | head -n 1)"

# 1) Flutter web build (idempotent; subsequent boots reuse the cache).
pushd flutter_app >/dev/null
  flutter --disable-analytics >/dev/null
  flutter pub get
  if [ ! -d web ] || [ ! -f web/index.html ]; then
    echo "[dd-dart-server] restoring missing flutter web template files"
    flutter create --project-name dd_dart_flutter_app --platforms=web --org dev.dd .
  fi
  mkdir -p web/vendor
  if [ ! -f web/vendor/htmx.min.js ]; then
    curl -fsSL https://unpkg.com/htmx.org@2.0.6/dist/htmx.min.js -o web/vendor/htmx.min.js
  fi
  if [ ! -f web/vendor/htmx-ext-ws.min.js ]; then
    curl -fsSL https://unpkg.com/htmx-ext-ws@2.0.4/ws.js -o web/vendor/htmx-ext-ws.min.js
  fi
  flutter build web \
    --release \
    --pwa-strategy=offline-first \
    --base-href=/dart/app/
popd >/dev/null

mkdir -p "$(dirname "${STATIC_DIR}")"
rm -rf "${STATIC_DIR}.new"
cp -a flutter_app/build/web "${STATIC_DIR}.new"
# Atomic swap so the running server never sees a half-written tree.
if [ -d "${STATIC_DIR}" ]; then
  rm -rf "${STATIC_DIR}.old"
  mv "${STATIC_DIR}" "${STATIC_DIR}.old"
fi
mv "${STATIC_DIR}.new" "${STATIC_DIR}"
rm -rf "${STATIC_DIR}.old"

# 2) Server build.
dart pub get
dart run build_runner build --delete-conflicting-outputs

# Dev mode: skip AOT, run via `dart run --enable-vm-service` so the
# in-process HotReloader can call reloadSources() on every isolate when
# .dart files under lib/ + bin/ change. Existing WebSockets stay open;
# the next render frame uses the new code.
#
# Set DEV_MODE=true (or HOT_RELOAD=true) on the deployment to switch.
DEV_MODE="${DEV_MODE:-false}"
if [ "${HOT_RELOAD:-false}" = "true" ]; then
  DEV_MODE=true
fi

export HTTP_HOST="${HTTP_HOST:-0.0.0.0}"
export HTTP_PORT="${HTTP_PORT:-8089}"
export STATIC_DIR="${STATIC_DIR}"

if [ "$DEV_MODE" = "true" ]; then
  echo "[dd-dart-server] DEV_MODE=true \u2192 JIT + hot reload"
  export HOT_RELOAD=true
  export HOT_RELOAD_PATHS="${HOT_RELOAD_PATHS:-lib,bin}"
  exec dart run \
    --enable-vm-service=8181 \
    --disable-service-auth-codes \
    --no-serve-devtools \
    bin/server.dart
fi

echo "[dd-dart-server] DEV_MODE=false \u2192 AOT (production, no hot reload)"
mkdir -p build
dart compile exe bin/server.dart -o build/dd-dart-server
exec build/dd-dart-server
