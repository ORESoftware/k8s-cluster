#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

cargo test --locked cli_flags::tests
cargo build --locked --no-default-features --bin dd-rust-network-mutex

binary="$repo_root/target/debug/dd-rust-network-mutex"
test -x "$binary"

scratch="$(mktemp -d)"
hostile_cwd="$scratch/hostile-cwd"
stdout_log="$scratch/stdout.log"
stderr_log="$scratch/stderr.log"
mkdir -p "$hostile_cwd"
trap 'rm -rf "$scratch"' EXIT

cat >"$hostile_cwd/.cli-flags.toml" <<'TOML'
[parse]
unknown_options_env = "LMX_CLI_UNKNOWN_OPTIONS"
errors_env = "LMX_CLI_PARSE_ERRORS"

[flags.attacker_owned]
env = "ATTACKER_OWNED"
aliases = ["attacker-owned"]
type = "string"
TOML

# A reviewed compile-time/package contract must win even when the process starts
# in a directory containing an attacker-controlled contract.
(
  cd "$hostile_cwd"
  env -u LMX_CLI_FLAGS_CONFIG "$binary" --help
) >"$stdout_log" 2>"$stderr_log"
grep -F -- "--tcp-port" "$stdout_log"
if grep -F -- "--attacker-owned" "$stdout_log" "$stderr_log"; then
  echo "hostile working-directory contract reached the help surface" >&2
  exit 1
fi

# Explicit selectors are operator trust decisions: invalid values fail closed
# and are never echoed to process output.
set +e
(
  cd "$hostile_cwd"
  LMX_CLI_FLAGS_CONFIG=attacker-runtime-secret.toml "$binary" --help
) >"$stdout_log" 2>"$stderr_log"
relative_status=$?
set -e
if [[ $relative_status -eq 0 ]]; then
  echo "relative explicit selector unexpectedly succeeded" >&2
  exit 1
fi
grep -F -- "LMX_CLI_FLAGS_CONFIG must be an absolute path" "$stderr_log"
if grep -F -- "attacker-runtime-secret.toml" "$stdout_log" "$stderr_log" \
  || grep -F -- "runtime-secret" "$stdout_log" "$stderr_log"; then
  echo "relative selector leaked into process output" >&2
  exit 1
fi

missing_selector="$scratch/missing-runtime-secret.toml"
set +e
LMX_CLI_FLAGS_CONFIG="$missing_selector" "$binary" --help \
  >"$stdout_log" 2>"$stderr_log"
missing_status=$?
set -e
if [[ $missing_status -eq 0 ]]; then
  echo "missing explicit selector unexpectedly succeeded" >&2
  exit 1
fi
grep -F -- "LMX_CLI_FLAGS_CONFIG does not name a readable regular file" "$stderr_log"
if grep -F -- "$missing_selector" "$stdout_log" "$stderr_log" \
  || grep -F -- "runtime-secret" "$stdout_log" "$stderr_log"; then
  echo "missing selector leaked into process output" >&2
  exit 1
fi

# Secret-bearing values remain environment-only. Supplying the old CLI aliases
# is an unknown-option error whose diagnostic contains no caller text.
rejected_value="postgres://runtime-secret@redacted.invalid/lmx"
set +e
LMX_CLI_FLAGS_CONFIG="$repo_root/.cli-flags.toml" \
  "$binary" "--auth-token=$rejected_value" \
  >"$stdout_log" 2>"$stderr_log"
unknown_status=$?
set -e
if [[ $unknown_status -eq 0 ]]; then
  echo "secret-bearing CLI flag unexpectedly succeeded" >&2
  exit 1
fi
grep -F -- "unknown broker CLI option" "$stderr_log"
if grep -F -- "$rejected_value" "$stdout_log" "$stderr_log" \
  || grep -F -- "runtime-secret" "$stdout_log" "$stderr_log"; then
  echo "unknown option value leaked into process output" >&2
  exit 1
fi

invalid_value="not-a-port-runtime-secret"
set +e
LMX_CLI_FLAGS_CONFIG="$repo_root/.cli-flags.toml" \
  "$binary" --tcp-port "$invalid_value" \
  >"$stdout_log" 2>"$stderr_log"
invalid_status=$?
set -e
if [[ $invalid_status -eq 0 ]]; then
  echo "invalid typed CLI flag unexpectedly succeeded" >&2
  exit 1
fi
grep -F -- "invalid broker CLI flag value" "$stderr_log"
if grep -F -- "$invalid_value" "$stdout_log" "$stderr_log" \
  || grep -F -- "runtime-secret" "$stdout_log" "$stderr_log"; then
  echo "invalid typed value leaked into process output" >&2
  exit 1
fi

for forbidden in 'current_dir(' 'find_upward_cli_flags_config'; do
  if grep -F -- "$forbidden" src/cli_flags.rs; then
    echo "forbidden ambient contract discovery remains: $forbidden" >&2
    exit 1
  fi
done

for secret_env in LMX_AUTH_TOKEN LMX_ADMIN_TOKEN LMX_TLS_KEY LMX_RAFT_PEER_TOKEN; do
  if grep -F -- "env = \"$secret_env\"" .cli-flags.toml; then
    echo "secret-bearing environment value remains exposed as a CLI flag: $secret_env" >&2
    exit 1
  fi
done

grep -F -- \
  'COPY .cli-flags.toml /etc/dd-rust-network-mutex/.cli-flags.toml' \
  Dockerfile
grep -F -- \
  'LMX_CLI_FLAGS_CONFIG=/etc/dd-rust-network-mutex/.cli-flags.toml' \
  Dockerfile

test "$(git hash-object vendor/flags2env/parser.c)" \
  = "2567d723ccdf9f0703dda5dfebac8ac2cb0ff2dd"
test "$(git hash-object vendor/flags2env/parser.h)" \
  = "56668698e50bafd8ce1dc49518a93bf9e564d7b5"

echo "live-mutex-rs trusted flags runtime smoke passed"
