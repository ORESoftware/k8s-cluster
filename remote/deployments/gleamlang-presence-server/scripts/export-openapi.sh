#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
service="$repo_root/remote/deployments/gleamlang-presence-server"
harness="$(mktemp -d "${TMPDIR:-/tmp}/gleam-presence-openapi.XXXXXX")"
cleanup() {
  find "$harness" -depth -delete
}
trap cleanup EXIT

mkdir -p "$harness/src/gleamlang_presence_server"
cp \
  "$service/src/gleamlang_presence_server/route_contract.gleam" \
  "$harness/src/gleamlang_presence_server/route_contract.gleam"
cp \
  "$service/src/gleamlang_presence_server/openapi_export.gleam" \
  "$harness/src/gleamlang_presence_server/openapi_export.gleam"
cat > "$harness/gleam.toml" <<'TOML'
name = "gleam_presence_contract"
version = "0.1.0"
target = "erlang"
gleam = ">= 1.17.0 and < 2.0.0"

[dependencies]
gleam_stdlib = ">= 0.68.0 and < 2.0.0"
gleam_http = ">= 4.0.0 and < 5.0.0"
gleam_json = ">= 2.0.0 and < 4.0.0"
TOML

cd "$harness"
gleam deps download >&2
gleam check >&2
gleam run -m gleamlang_presence_server/openapi_export
