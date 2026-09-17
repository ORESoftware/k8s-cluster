#!/usr/bin/env bash
# Run the protocol checks and smoke tests for every cross-runtime client.
# Live smoke tests run against a local broker; protocol-only seeds are checked
# offline before the broker probe.
#
# Prerequisites: cargo, node, npm, go, gleam, python3, a C++17 compiler, a JDK
# 17+ (javac), erlang/erlc, ocamlc, dotnet, and either dart or docker (the dart
# smoke runs in `docker run dart:stable …` if no local SDK is available).
# Missing optional toolchains are skipped with a notice rather than failing the
# run.
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

echo "==> client protocol parity"
"$HERE/clients/check-protocol-parity.sh"

echo "==> erlang protocol"
if command -v erl >/dev/null 2>&1 && command -v erlc >/dev/null 2>&1; then
  ( cd "$HERE/clients/erlang" && make test )
else
  echo "(skipping erlang: erl/erlc not found)"
fi

echo "==> elixir protocol"
if command -v mix >/dev/null 2>&1; then
  ( cd "$HERE/clients/elixir" && mix test )
else
  echo "(skipping elixir: mix not found)"
fi

echo "==> ocaml protocol"
if command -v ocamlc >/dev/null 2>&1; then
  ( cd "$HERE/clients/ocaml" && make test )
else
  echo "(skipping ocaml: ocamlc not found)"
fi

echo "==> c# protocol"
if command -v dotnet >/dev/null 2>&1; then
  dotnet run --project "$HERE/clients/csharp"
else
  echo "(skipping c#: dotnet not found)"
fi

echo "==> f# protocol"
if command -v dotnet >/dev/null 2>&1; then
  dotnet run --project "$HERE/clients/fsharp"
else
  echo "(skipping f#: dotnet not found)"
fi

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

echo "==> python smoke"
if command -v python3 >/dev/null 2>&1; then
  ( cd "$HERE/clients/python" && python3 -m unittest discover -s tests -p 'test_*.py' -q && python3 smoke.py )
else
  echo "(skipping python: python3 not found)"
fi

echo "==> c++ smoke"
if command -v c++ >/dev/null 2>&1 || command -v g++ >/dev/null 2>&1; then
  ( cd "$HERE/clients/cpp" && make test >/dev/null && make run )
else
  echo "(skipping c++: no C++ compiler found)"
fi

echo "==> java smoke"
JAVAC_BIN="${JAVAC:-javac}"
if ! command -v "$JAVAC_BIN" >/dev/null 2>&1 && [ -x /opt/homebrew/opt/openjdk@17/bin/javac ]; then
  export PATH="/opt/homebrew/opt/openjdk@17/bin:$PATH"
fi
if command -v javac >/dev/null 2>&1; then
  ( cd "$HERE/clients/java" && ./build.sh >/dev/null \
      && java -cp out com.oresoftware.networkmutex.ProtocolTest \
      && java -cp out com.oresoftware.networkmutex.Smoke )
else
  echo "(skipping java: javac not found; install a JDK 17+)"
fi

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

echo "==> shell smoke"
"$HERE/clients/shell/smoke.sh"

echo "==> powershell smoke"
if command -v pwsh >/dev/null 2>&1; then
  pwsh "$HERE/clients/powershell/smoke.ps1"
else
  echo "(skipping powershell: pwsh not found)"
fi

echo
echo "==> all client smokes OK"
