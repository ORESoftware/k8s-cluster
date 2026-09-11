#!/usr/bin/env bash
# Verify every cross-runtime client mirrors the Rust wire discriminators.
#
# The canonical request/response variants live in ../src/protocol.rs. This
# script converts the Rust enum variant names into serde's camelCase wire
# values, then checks each client protocol mirror for every value.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROTOCOL_RS="$ROOT/src/protocol.rs"

camel_case() {
  local name="$1"
  local first rest
  first="$(printf '%s' "${name:0:1}" | tr '[:upper:]' '[:lower:]')"
  rest="${name:1}"
  printf '%s%s' "$first" "$rest"
}

enum_variants() {
  local enum_name="$1"
  awk -v enum_name="$enum_name" '
    $0 ~ "pub enum " enum_name " " { in_enum = 1; next }
    in_enum && $0 ~ /^}/ { exit }
    in_enum {
      line = $0
      sub(/^[[:space:]]*/, "", line)
      if (line ~ /^[A-Z][A-Za-z0-9_]*[[:space:]]*(\{|,)/) {
        sub(/[[:space:]]*(\{|,).*/, "", line)
        print line
      }
    }
  ' "$PROTOCOL_RS"
}

REQUEST_TYPES=()
while IFS= read -r variant; do
  REQUEST_TYPES+=("$(camel_case "$variant")")
done < <(enum_variants Request)

RESPONSE_TYPES=()
while IFS= read -r variant; do
  RESPONSE_TYPES+=("$(camel_case "$variant")")
done < <(enum_variants Response)

if (( ${#REQUEST_TYPES[@]} == 0 || ${#RESPONSE_TYPES[@]} == 0 )); then
  printf 'FAIL: could not derive request/response variants from %s\n' "$PROTOCOL_RS" >&2
  exit 1
fi

CLIENT_PROTOCOLS=(
  "TypeScript:clients/ts/src/protocol.ts"
  "Go:clients/go/protocol.go"
  "Dart:clients/dart/lib/protocol.dart"
  "Gleam:clients/gleam/src/dd_rust_network_mutex_client/protocol.gleam"
  "Python:clients/python/network_mutex/protocol.py"
  "C++:clients/cpp/include/network_mutex/protocol.hpp"
  "Java:clients/java/src/main/java/com/oresoftware/networkmutex/Protocol.java"
  "Erlang:clients/erlang/src/network_mutex_protocol.erl"
  "Elixir:clients/elixir/lib/network_mutex/protocol.ex"
  "OCaml:clients/ocaml/network_mutex_protocol.ml"
  "C#:clients/csharp/Protocol.cs"
  "F#:clients/fsharp/Protocol.fs"
  "Shell:clients/shell/live_mutex_client.sh"
  "PowerShell:clients/powershell/LiveMutexClient.ps1"
)

failures=0

check_file_contains() {
  local lang="$1"
  local file="$2"
  local kind="$3"
  local wire="$4"

  if ! grep -Fq "$wire" "$ROOT/$file"; then
    printf 'FAIL: %s %s mirror is missing wire value %q in %s\n' \
      "$lang" "$kind" "$wire" "$file" >&2
    failures=$((failures + 1))
  fi
}

for entry in "${CLIENT_PROTOCOLS[@]}"; do
  lang="${entry%%:*}"
  file="${entry#*:}"

  if [[ ! -f "$ROOT/$file" ]]; then
    printf 'FAIL: %s protocol file missing: %s\n' "$lang" "$file" >&2
    failures=$((failures + 1))
    continue
  fi

  for wire in "${REQUEST_TYPES[@]}"; do
    check_file_contains "$lang" "$file" "request" "$wire"
  done
  for wire in "${RESPONSE_TYPES[@]}"; do
    check_file_contains "$lang" "$file" "response" "$wire"
  done
done

if (( failures > 0 )); then
  printf '\nprotocol parity check failed with %d issue(s)\n' "$failures" >&2
  exit 1
fi

printf 'protocol parity OK: %d request types and %d response types mirrored by %d client languages\n' \
  "${#REQUEST_TYPES[@]}" "${#RESPONSE_TYPES[@]}" "${#CLIENT_PROTOCOLS[@]}"
