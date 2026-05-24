#!/usr/bin/env bash
# Run the smoke test for every cross-runtime client (TS / Rust / Go / Dart /
# Gleam) against a local broker. Each runtime exits non-zero on failure.
#
# Prerequisites: cargo, node, npm, go, gleam, and either dart or docker
# (the dart smoke runs in `docker run dart:stable …` if no local SDK is
# available).
#
#   ./scripts/run-all-client-smokes.sh
#
# This script does *not* spawn the broker; bring one up first, e.g.
#   cargo run --release --no-default-features --bin dd-rust-network-mutex
# (defaults to listening on TCP 127.0.0.1:6970, HTTP :6971).

set -euo pipefail

HERE="$(cd "$(dirname "$0")/.." && pwd)"
HOST="${LIVE_MUTEX_HOST:-127.0.0.1}"
PORT="${LIVE_MUTEX_PORT:-6970}"
export LIVE_MUTEX_HOST="$HOST"
export LIVE_MUTEX_PORT="$PORT"

if ! nc -z "$HOST" "$PORT" 2>/dev/null; then
  echo "FATAL: broker not listening on $HOST:$PORT" >&2
  echo "       start it with: cargo run --release --no-default-features --bin dd-rust-network-mutex" >&2
  exit 1
fi

echo "==> rust client integration tests"
( cd "$HERE" && cargo test --no-default-features --tests --quiet 2>&1 | tail -10 )

echo "==> typescript smoke"
( cd "$HERE/clients/ts" && npx tsx src/smoke.ts )

echo "==> go smoke"
( cd "$HERE/clients/go" && go run ./cmd/smoke )

echo "==> gleam smoke"
( cd "$HERE/clients/gleam" && LIVE_MUTEX_SMOKE=1 gleam test 2>&1 | grep -E '\[smoke-gleam\]|passed|failures' )

echo "==> dart smoke"
if command -v dart >/dev/null 2>&1; then
  ( cd "$HERE/clients/dart" && dart pub get >/dev/null && dart run bin/smoke.dart )
elif command -v docker >/dev/null 2>&1; then
  echo "(no local dart SDK; running in dart:stable container)"
  docker run --rm --network=host \
    -v "$HERE/clients/dart:/work" -w /work \
    -e "LIVE_MUTEX_HOST=host.docker.internal" \
    -e "LIVE_MUTEX_PORT=$PORT" \
    dart:stable sh -c "dart pub get >/dev/null && dart run bin/smoke.dart"
else
  echo "(skipping dart: neither dart SDK nor docker available)"
fi

echo
echo "==> all client smokes OK"
