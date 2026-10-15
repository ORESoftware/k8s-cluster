#!/usr/bin/env bash
# Run the upstream live-mutex#22 socket-tuning experiment inside a Linux
# container so TCP_QUICKACK actually fires (it's Linux-only). On
# macOS hosts this script self-bootstraps via `docker run rust:1.90`;
# on Linux hosts you can run the inner block directly.
#
# What it does:
#
# 1. Spawns broker A (LMX_TCP_QUICKACK=true)  on :6970
# 2. Spawns broker B (LMX_TCP_QUICKACK=false) on :6980
# 3. Runs the TS latency probe against each.
# 4. Dumps both /metrics endpoints so you can confirm the
#    `tcp_quickack_applied_total` counter actually moved on the
#    QUICKACK-on broker.
#
# Run:    ./scripts/quickack-experiment.sh
set -euo pipefail

HERE="$(cd "$(dirname "$0")/.." && pwd)"

if [[ "${INSIDE_CONTAINER:-}" != "1" ]]; then
  exec docker run --rm \
    -v "$HERE":/work -w /work \
    -e INSIDE_CONTAINER=1 \
    rust:1.90-bookworm \
    sh -c 'apt-get update -qq >/dev/null && apt-get install -qq -y -o Dpkg::Use-Pty=0 curl netcat-openbsd ca-certificates >/dev/null && curl -fsSL https://deb.nodesource.com/setup_22.x | bash - >/dev/null 2>&1 && apt-get install -qq -y -o Dpkg::Use-Pty=0 nodejs >/dev/null && bash scripts/quickack-experiment.sh'
fi

cd "$HERE"
echo "==> building broker (linux release)"
cargo build --release --no-default-features >/dev/null 2>&1
BIN="$HERE/target/release/dd-rust-network-mutex"

echo "==> spawning broker A (NODELAY=true QUICKACK=true) on :6970"
LMX_TCP_PORT=6970 LMX_HTTP_PORT=6971 LMX_TCP_NODELAY=true LMX_TCP_QUICKACK=true \
  "$BIN" >/tmp/broker-a.log 2>&1 &
PID_A=$!

echo "==> spawning broker B (NODELAY=false QUICKACK=false) on :6980"
LMX_TCP_PORT=6980 LMX_HTTP_PORT=6981 LMX_TCP_NODELAY=false LMX_TCP_QUICKACK=false \
  "$BIN" >/tmp/broker-b.log 2>&1 &
PID_B=$!

cleanup() {
  kill "$PID_A" "$PID_B" 2>/dev/null || true
  wait 2>/dev/null || true
}
trap cleanup EXIT

# Wait for both to bind.
for _ in $(seq 1 50); do
  if nc -z 127.0.0.1 6970 && nc -z 127.0.0.1 6980; then
    break
  fi
  sleep 0.1
done

cd "$HERE/clients/ts"
# The host's node_modules might be from a different platform (darwin vs
# linux) when this script self-bootstraps a container, so esbuild's
# native binary won't match. Use a separate per-platform install
# directory instead of fighting the host's node_modules.
ARCH_NM="/tmp/lmx-quickack-nm-$(uname -s)-$(uname -m)"
mkdir -p "$ARCH_NM"
if [[ ! -d "$ARCH_NM/node_modules" ]]; then
  echo "==> installing node deps for $(uname -s)-$(uname -m)"
  cp package.json package-lock.json tsconfig.json "$ARCH_NM/"
  ( cd "$ARCH_NM" && npm install --silent --no-audit --no-fund )
fi
export NODE_PATH="$ARCH_NM/node_modules"

echo
echo "============================================================"
echo "BROKER A — NODELAY=true,  QUICKACK=true"
echo "============================================================"
PORT=6970 LABEL="A: tuning-ON " ITERATIONS="${ITERATIONS:-2000}" \
  "$ARCH_NM/node_modules/.bin/tsx" src/latency_probe.ts

echo
echo "============================================================"
echo "BROKER B — NODELAY=false, QUICKACK=false"
echo "============================================================"
PORT=6980 LABEL="B: tuning-OFF" ITERATIONS="${ITERATIONS:-2000}" \
  "$ARCH_NM/node_modules/.bin/tsx" src/latency_probe.ts

echo
echo "============================================================"
echo "Prometheus counters (post-probe)"
echo "============================================================"
echo "BROKER A:"
curl -fsS http://127.0.0.1:6971/metrics | grep -E 'tcp_(nodelay|quickack)_applied_total ' || true
echo "BROKER B:"
curl -fsS http://127.0.0.1:6981/metrics | grep -E 'tcp_(nodelay|quickack)_applied_total ' || true
