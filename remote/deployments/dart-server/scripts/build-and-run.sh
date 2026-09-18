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
#   FLUTTER_HOME       /opt/flutter
#   DART_HOME          ${FLUTTER_HOME}/bin/cache/dart-sdk
#   STATIC_DIR         ${PWD}/public           — flutter_app/ bundle, /dart/app/
#   MOBILE_STATIC_DIR  ${PWD}/mobile-public    — flutter_mobile_app/ bundle, /dart/mobile/
#   HTTP_HOST          0.0.0.0
#   HTTP_PORT          8089

set -euo pipefail

FLUTTER_HOME="${FLUTTER_HOME:-/opt/flutter}"
DART_HOME="${DART_HOME:-${FLUTTER_HOME}/bin/cache/dart-sdk}"
STATIC_DIR="${STATIC_DIR:-${PWD}/public}"
MOBILE_STATIC_DIR="${MOBILE_STATIC_DIR:-${PWD}/mobile-public}"

export PATH="${FLUTTER_HOME}/bin:${DART_HOME}/bin:${PATH}"

echo "[dd-dart-server] dart $(dart --version 2>&1)"

# 1) Flutter web build for the SPA (/dart/app/) and the mobile bundle
# (/dart/mobile/). These are intentionally skippable so a broken Flutter
# compile cannot block the Jaspr SSR pages or the WSS endpoint.
#
# Toggle via SKIP_FLUTTER_BUILD on the deployment:
#   SKIP_FLUTTER_BUILD=true   (default) — skip both Flutter builds, drop
#                             a small placeholder index.html into the
#                             static dirs so /dart/app/ and /dart/mobile/
#                             return something instead of 404.
#   SKIP_FLUTTER_BUILD=false  — run the full `flutter build web` for both
#                             projects and atomically swap the output
#                             into the static dirs.
SKIP_FLUTTER_BUILD="${SKIP_FLUTTER_BUILD:-true}"

place_flutter_placeholder() {
  local target="$1"
  local label="$2"
  local href="$3"
  mkdir -p "$(dirname "${target}")"
  rm -rf "${target}.new"
  mkdir -p "${target}.new"
  cat >"${target}.new/index.html" <<EOF
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>${label} — coming soon</title>
<base href="${href}">
<style>
body { font-family: ui-sans-serif, system-ui, sans-serif; background:#0d1117; color:#c9d1d9; padding:32px; }
a { color:#58a6ff; }
.box { max-width:560px; border:1px solid #30363d; border-radius:8px; padding:20px; }
</style>
</head>
<body>
<div class="box">
<h1>${label}</h1>
<p>This bundle is not yet built. The Jaspr SSR pages and the WSS endpoint are still up:</p>
<ul>
<li><a href="/dart/pages/about">/dart/pages/about</a></li>
<li><a href="/dart/pages">/dart/pages</a></li>
</ul>
<p>Set <code>SKIP_FLUTTER_BUILD=false</code> on the deployment and roll out to enable this surface.</p>
</div>
</body>
</html>
EOF
  if [ -d "${target}" ]; then
    rm -rf "${target}.old"
    mv "${target}" "${target}.old"
  fi
  mv "${target}.new" "${target}"
  rm -rf "${target}.old"
}

if [ "${SKIP_FLUTTER_BUILD}" = "true" ]; then
  echo "[dd-dart-server] SKIP_FLUTTER_BUILD=true — skipping flutter build, writing placeholders"
  place_flutter_placeholder "${STATIC_DIR}"        "dart-server SPA"          "/dart/app/"
  place_flutter_placeholder "${MOBILE_STATIC_DIR}" "dart-server mobile bundle" "/dart/mobile/"
else
  echo "[dd-dart-server] flutter $(flutter --version 2>&1 | head -n 1)"

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
  if [ -d "${STATIC_DIR}" ]; then
    rm -rf "${STATIC_DIR}.old"
    mv "${STATIC_DIR}" "${STATIC_DIR}.old"
  fi
  mv "${STATIC_DIR}.new" "${STATIC_DIR}"
  rm -rf "${STATIC_DIR}.old"

  if [ -d flutter_mobile_app ]; then
    pushd flutter_mobile_app >/dev/null
      flutter --disable-analytics >/dev/null
      flutter pub get
      if [ ! -d web ] || [ ! -f web/index.html ]; then
        echo "[dd-dart-server] restoring missing flutter mobile web template files"
        flutter create --project-name dd_dart_flutter_mobile_app --platforms=web --org dev.dd .
      fi
      flutter build web \
        --release \
        --pwa-strategy=offline-first \
        --base-href=/dart/mobile/
    popd >/dev/null

    mkdir -p "$(dirname "${MOBILE_STATIC_DIR}")"
    rm -rf "${MOBILE_STATIC_DIR}.new"
    cp -a flutter_mobile_app/build/web "${MOBILE_STATIC_DIR}.new"
    if [ -d "${MOBILE_STATIC_DIR}" ]; then
      rm -rf "${MOBILE_STATIC_DIR}.old"
      mv "${MOBILE_STATIC_DIR}" "${MOBILE_STATIC_DIR}.old"
    fi
    mv "${MOBILE_STATIC_DIR}.new" "${MOBILE_STATIC_DIR}"
    rm -rf "${MOBILE_STATIC_DIR}.old"
  else
    place_flutter_placeholder "${MOBILE_STATIC_DIR}" "dart-server mobile bundle" "/dart/mobile/"
  fi
fi

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
export MOBILE_STATIC_DIR="${MOBILE_STATIC_DIR}"

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
