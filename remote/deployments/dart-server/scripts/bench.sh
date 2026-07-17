#!/usr/bin/env bash
# Drive the HTTP + WSS load testers against a running dd-dart-server.
#
# Usage:
#   scripts/bench.sh                    # default 30s test
#   BENCH_DURATION=120 scripts/bench.sh # longer pass
#   BENCH_HOST=10.0.0.7 scripts/bench.sh
#
# Output:
#   bench-results.json — last results (overwritten each run)
#   bench-results.<unix>.json — historical archive
#
# This script does not start the server. Run it against a server you've
# already booted (scripts/dev.sh for JIT + hot reload, or the AOT
# binary).

set -euo pipefail

cd "$(dirname "$0")/.."

HOST="${BENCH_HOST:-127.0.0.1}"
PORT="${BENCH_PORT:-8089}"
DURATION="${BENCH_DURATION:-30}"
WARMUP="${BENCH_WARMUP:-3}"
HTTP_CONNS="${BENCH_HTTP_CONNS:-32}"
WSS_CONNS="${BENCH_WSS_CONNS:-128}"
WSS_RATE="${BENCH_WSS_RATE:-50}"

# Sanity: server must answer /healthz before we run the WSS test, or
# every connection attempt will fail confusingly.
if ! curl -fsS "http://${HOST}:${PORT}/healthz" >/dev/null; then
  echo "[bench.sh] ${HOST}:${PORT}/healthz did not respond — start the server first"
  exit 1
fi

echo "[bench.sh] target http://${HOST}:${PORT}, duration=${DURATION}s warmup=${WARMUP}s"

ts="$(date +%s)"
results_file="bench-results.${ts}.json"

{
  echo "["

  echo "  $(BENCH_URL=http://${HOST}:${PORT}/healthz \
        BENCH_CONNS=${HTTP_CONNS} \
        BENCH_DURATION=${DURATION} \
        BENCH_WARMUP=${WARMUP} \
        dart run tools/http_loadtest.dart),"

  echo "  $(BENCH_URL=http://${HOST}:${PORT}/dart/pages \
        BENCH_CONNS=${HTTP_CONNS} \
        BENCH_DURATION=${DURATION} \
        BENCH_WARMUP=${WARMUP} \
        dart run tools/http_loadtest.dart),"

  echo "  $(BENCH_WSS_URL=ws://${HOST}:${PORT}/dart/wss \
        BENCH_WSS_CONNS=${WSS_CONNS} \
        BENCH_WSS_RATE=${WSS_RATE} \
        BENCH_WSS_DURATION=${DURATION} \
        BENCH_WSS_WARMUP=${WARMUP} \
        BENCH_WSS_TRIGGER=bump \
        dart run tools/wss_loadtest.dart),"

  echo "  $(BENCH_WSS_URL=ws://${HOST}:${PORT}/dart/wss \
        BENCH_WSS_CONNS=${WSS_CONNS} \
        BENCH_WSS_RATE=${WSS_RATE} \
        BENCH_WSS_DURATION=${DURATION} \
        BENCH_WSS_WARMUP=${WARMUP} \
        BENCH_WSS_TRIGGER=say \
        dart run tools/wss_loadtest.dart)"

  echo "]"
} | tee "${results_file}" > bench-results.json

echo "[bench.sh] wrote ${results_file} and bench-results.json"
echo "[bench.sh] live metrics: curl -s http://${HOST}:${PORT}/metrics | rg dart_"
