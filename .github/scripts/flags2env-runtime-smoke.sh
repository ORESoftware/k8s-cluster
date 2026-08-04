#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

cargo test --locked flags::tests
cargo build --locked --bin threefa-sync-server

binary="$repo_root/target/debug/threefa-sync-server"
hostile_cwd="$(mktemp -d)"
stdout_log="$(mktemp)"
stderr_log="$(mktemp)"
trap 'rm -rf "$hostile_cwd" "$stdout_log" "$stderr_log"' EXIT

cat >"$hostile_cwd/.cli-flags.toml" <<'TOML'
[parse]
allow_unknown = true

[flags.database-url]
env = "DATABASE_URL"
aliases = ["database-url"]
type = "string"
TOML

set +e
(
  cd "$hostile_cwd"
  "$binary" --database-url=postgres://runtime-secret@redacted.invalid/threefa
) >"$stdout_log" 2>"$stderr_log"
status=$?
set -e

test "$status" -eq 2
grep -F -- "--database-url=<redacted>" "$stderr_log"
if grep -F -- "runtime-secret" "$stdout_log" "$stderr_log"; then
  echo "rejected secret-bearing option value leaked into process output" >&2
  exit 1
fi

set +e
(
  cd "$hostile_cwd"
  THREEFA_FLAGS_CONFIG=reviewed.toml "$binary" --bind-addr=127.0.0.1:18080
) >"$stdout_log" 2>"$stderr_log"
relative_status=$?
set -e

test "$relative_status" -eq 2
grep -F -- "THREEFA_FLAGS_CONFIG must be an absolute path" "$stderr_log"
if grep -F -- "reviewed.toml" "$stdout_log" "$stderr_log"; then
  echo "relative explicit contract path leaked into process output" >&2
  exit 1
fi

set +e
THREEFA_FLAGS_CONFIG="$hostile_cwd/missing.toml" \
  "$binary" --bind-addr=127.0.0.1:18080 \
  >"$stdout_log" 2>"$stderr_log"
missing_status=$?
set -e

test "$missing_status" -eq 2
grep -F -- "THREEFA_FLAGS_CONFIG does not name a readable regular file" "$stderr_log"
if grep -F -- "$hostile_cwd/missing.toml" "$stdout_log" "$stderr_log"; then
  echo "explicit contract path leaked into process output" >&2
  exit 1
fi

echo "threefa-sync-server trusted flags2env runtime smoke passed"
