#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

cargo test --locked runtime_config::tests
cargo build --locked --bin dd-in-house-mip-solver-node

binary="$repo_root/target/debug/dd-in-house-mip-solver-node"
test -x "$binary"

scratch="$(mktemp -d)"
hostile_cwd="$scratch/hostile-cwd"
hostile_bin="$scratch/hostile-bin"
stdout_log="$scratch/stdout.log"
stderr_log="$scratch/stderr.log"
parser_marker="$scratch/ambient-parser-executed"
mkdir -p "$hostile_cwd" "$hostile_bin"
trap 'rm -rf "$scratch"' EXIT

cat >"$hostile_cwd/.cli-flags.toml" <<'TOML'
[parse]
allow_unknown = true

[flags.attacker_owned]
env = "ATTACKER_OWNED"
aliases = ["attacker-owned"]
type = "string"
TOML

cat >"$hostile_bin/flags2env" <<SH
#!/usr/bin/env bash
printf 'ambient parser executed\n' >'$parser_marker'
exit 0
SH
chmod +x "$hostile_bin/flags2env"

# A hostile working directory, PATH entry, and legacy fallback variables cannot
# replace either the reviewed policy or the statically linked parser.
(
  cd "$hostile_cwd"
  env -u FLAGS2ENV_CONFIG \
    PATH="$hostile_bin:$PATH" \
    FLAGS2ENV_BIN="$hostile_bin/flags2env" \
    FLAGS2ENV_NATIVE_LIB="$hostile_cwd/libflags2env.so" \
    "$binary" --help
) >"$stdout_log" 2>"$stderr_log"
grep -F -- "--port" "$stdout_log"
if grep -F -- "--attacker-owned" "$stdout_log" "$stderr_log"; then
  echo "hostile working-directory contract reached the help surface" >&2
  exit 1
fi
if [[ -e "$parser_marker" ]]; then
  echo "PATH-selected flags2env process executed" >&2
  exit 1
fi

# Explicit selectors are operator trust decisions: invalid values must fail
# closed and must not be echoed back to logs.
set +e
(
  cd "$hostile_cwd"
  FLAGS2ENV_CONFIG=reviewed-runtime-secret.toml "$binary" --help
) >"$stdout_log" 2>"$stderr_log"
relative_status=$?
set -e
if [[ $relative_status -ne 2 ]]; then
  echo "relative selector returned unexpected status: $relative_status" >&2
  cat "$stderr_log" >&2
  exit 1
fi
grep -F -- "FLAGS2ENV_CONFIG must be an absolute path" "$stderr_log"
if grep -F -- "reviewed-runtime-secret.toml" "$stdout_log" "$stderr_log"; then
  echo "relative selector leaked into process output" >&2
  exit 1
fi

missing_selector="$scratch/missing-runtime-secret.toml"
set +e
(
  cd "$hostile_cwd"
  FLAGS2ENV_CONFIG="$missing_selector" "$binary" --help
) >"$stdout_log" 2>"$stderr_log"
missing_status=$?
set -e
if [[ $missing_status -ne 2 ]]; then
  echo "missing selector returned unexpected status: $missing_status" >&2
  cat "$stderr_log" >&2
  exit 1
fi
grep -F -- "FLAGS2ENV_CONFIG does not name a readable regular file" "$stderr_log"
if grep -F -- "$missing_selector" "$stdout_log" "$stderr_log" \
  || grep -F -- "runtime-secret" "$stdout_log" "$stderr_log"; then
  echo "missing selector leaked into process output" >&2
  exit 1
fi

rejected_value="postgres://runtime-secret@redacted.invalid/mip"
set +e
"$binary" \
  "--definitely-not-a-real-flag=$rejected_value" \
  >"$stdout_log" 2>"$stderr_log"
unknown_status=$?
set -e
if [[ $unknown_status -ne 2 ]]; then
  echo "unknown option returned unexpected status: $unknown_status" >&2
  cat "$stderr_log" >&2
  exit 1
fi
grep -F -- "flags2env rejected one or more unknown CLI options" "$stderr_log"
if grep -F -- "$rejected_value" "$stdout_log" "$stderr_log" \
  || grep -F -- "runtime-secret" "$stdout_log" "$stderr_log"; then
  echo "unknown option value leaked into process output" >&2
  exit 1
fi

invalid_value="not-a-number-runtime-secret"
set +e
"$binary" \
  "--max-nodes=$invalid_value" \
  >"$stdout_log" 2>"$stderr_log"
invalid_status=$?
set -e
if [[ $invalid_status -ne 2 ]]; then
  echo "invalid typed option returned unexpected status: $invalid_status" >&2
  cat "$stderr_log" >&2
  exit 1
fi
grep -F -- "flags2env rejected one or more CLI values" "$stderr_log"
if grep -F -- "$invalid_value" "$stdout_log" "$stderr_log" \
  || grep -F -- "runtime-secret" "$stdout_log" "$stderr_log"; then
  echo "invalid option value leaked into process output" >&2
  exit 1
fi

for forbidden in \
  'current_dir(' \
  'Library::new' \
  'Command::new' \
  'parse_with_native_library' \
  'parse_with_cli_binary' \
  'FLAGS2ENV_NATIVE_LIB' \
  'FLAGS2ENV_BIN'
do
  if grep -F -- "$forbidden" src/runtime_config.rs; then
    echo "forbidden ambient flags fallback remains: $forbidden" >&2
    exit 1
  fi
done

grep -F -- \
  'FLAGS2ENV_CONFIG=/usr/local/share/dd-in-house-mip-solver-node/.cli-flags.toml' \
  Dockerfile
grep -F -- \
  '/usr/local/share/dd-in-house-mip-solver-node/.cli-flags.toml' \
  Dockerfile
if grep -F -- '/app/.cli-flags.toml' Dockerfile; then
  echo "container still packages policy under the working directory" >&2
  exit 1
fi

test "$(git hash-object third_party/flags2env/src/parser.c)" \
  = "2567d723ccdf9f0703dda5dfebac8ac2cb0ff2dd"
test "$(git hash-object third_party/flags2env/src/parser.h)" \
  = "56668698e50bafd8ce1dc49518a93bf9e564d7b5"

echo "mip solver trusted flags2env runtime smoke passed"
