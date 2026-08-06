#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
contract_dir="$repo_root/tests/interface-wasm-browser"
chrome_bin=${CHROME_BIN:-}
artifact_dir=${CANONICAL_BROWSER_ARTIFACT_DIR:-}
wall_timeout=${CANONICAL_INTERFACE_BROWSER_TIMEOUT_SECONDS:-45}
port=${CANONICAL_INTERFACE_BROWSER_PORT:-4181}

if [[ ! "$wall_timeout" =~ ^[1-9][0-9]*$ ]] || (( wall_timeout > 120 )); then
  echo "CANONICAL_INTERFACE_BROWSER_TIMEOUT_SECONDS must be an integer from 1 to 120" >&2
  exit 1
fi
if [[ ! "$port" =~ ^[0-9]+$ ]] || (( port < 1024 || port > 65534 )); then
  echo "CANONICAL_INTERFACE_BROWSER_PORT must be an integer from 1024 to 65534" >&2
  exit 1
fi
debug_port=${CANONICAL_INTERFACE_BROWSER_DEBUG_PORT:-$((port + 1))}
if [[ ! "$debug_port" =~ ^[0-9]+$ ]] || (( debug_port < 1024 || debug_port > 65535 )); then
  echo "CANONICAL_INTERFACE_BROWSER_DEBUG_PORT must be an integer from 1024 to 65535" >&2
  exit 1
fi
if [[ "$debug_port" == "$port" ]]; then
  echo "Browser contract and DevTools ports must differ" >&2
  exit 1
fi

if [[ -z "$chrome_bin" ]]; then
  for candidate in google-chrome google-chrome-stable chromium chromium-browser; do
    if command -v "$candidate" >/dev/null 2>&1; then
      chrome_bin=$(command -v "$candidate")
      break
    fi
  done
fi

if [[ -z "$chrome_bin" || ! -x "$chrome_bin" ]]; then
  echo "A Chromium-family browser is required for the interface WASM contract" >&2
  exit 1
fi

for command in timeout setsid node python3 curl; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "$command is required for the wall-clock Chromium contract" >&2
    exit 1
  fi
done

for required in \
  "$contract_dir/index.html" \
  "$contract_dir/contract.mjs" \
  "$contract_dir/driver.mjs" \
  "$repo_root/generated/rust-wasm/pkg/canonical_interfaces_wasm.js" \
  "$repo_root/generated/rust-wasm/pkg/canonical_interfaces_wasm_bg.wasm" \
  "$repo_root/generated/rust-wasm/pkg/canonical_interfaces_wasm.d.ts"
do
  if [[ ! -f "$required" ]]; then
    echo "Missing browser contract input: $required" >&2
    exit 1
  fi
done

work_dir=$(mktemp -d)
server_log="$work_dir/server.log"
chrome_log="$work_dir/chrome.stderr"
chrome_stdout="$work_dir/chrome.stdout"
driver_log="$work_dir/driver.log"
dom="$work_dir/result.html"
result_json="$work_dir/result.json"
server_pid=
chrome_pid=

preserve_failure_evidence() {
  if [[ -z "$artifact_dir" ]]; then
    return
  fi
  mkdir -p "$artifact_dir"
  for evidence in \
    "$dom" \
    "$result_json" \
    "$driver_log" \
    "$server_log" \
    "$chrome_log" \
    "$chrome_stdout"
  do
    if [[ -f "$evidence" ]]; then
      cp "$evidence" "$artifact_dir/$(basename "$evidence")"
    fi
  done
}

cleanup() {
  local status=$?
  trap - EXIT
  set +e

  if [[ -n "$chrome_pid" ]]; then
    # Chromium owns several descendants that can keep mutating the profile
    # after the browser leader exits. Launching it under setsid lets cleanup
    # terminate the complete isolated process group instead of racing those
    # descendants or leaking them into the next certification attempt.
    kill -TERM -- "-$chrome_pid" >/dev/null 2>&1 || true
    for _ in $(seq 1 50); do
      if ! kill -0 -- "-$chrome_pid" >/dev/null 2>&1; then
        break
      fi
      sleep 0.1
    done
    kill -KILL -- "-$chrome_pid" >/dev/null 2>&1 || true
    wait "$chrome_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$server_pid" ]]; then
    kill "$server_pid" >/dev/null 2>&1 || true
    wait "$server_pid" >/dev/null 2>&1 || true
  fi

  for _ in $(seq 1 10); do
    if rm -rf -- "$work_dir" >/dev/null 2>&1; then
      exit "$status"
    fi
    sleep 0.1
  done
  echo "warning: browser work directory could not be removed: $work_dir" >&2
  exit "$status"
}
trap cleanup EXIT

print_failure_evidence() {
  echo "--- DevTools driver ---" >&2
  sed -n '1,200p' "$driver_log" >&2 || true
  echo "--- DOM snapshot ---" >&2
  sed -n '1,240p' "$dom" >&2 || true
  echo "--- contract result ---" >&2
  sed -n '1,160p' "$result_json" >&2 || true
  echo "--- Chromium stderr ---" >&2
  sed -n '1,160p' "$chrome_log" >&2 || true
  echo "--- local server log ---" >&2
  sed -n '1,160p' "$server_log" >&2 || true
}

python3 -m http.server "$port" \
  --bind 127.0.0.1 \
  --directory "$repo_root" \
  >"$server_log" 2>&1 &
server_pid=$!

for _ in $(seq 1 100); do
  if curl --fail --silent \
    "http://127.0.0.1:${port}/tests/interface-wasm-browser/index.html" \
    >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$server_pid" >/dev/null 2>&1; then
    preserve_failure_evidence
    print_failure_evidence
    exit 1
  fi
  sleep 0.1
done

if ! curl --fail --silent --show-error \
  "http://127.0.0.1:${port}/tests/interface-wasm-browser/index.html" >/dev/null; then
  preserve_failure_evidence
  print_failure_evidence
  echo "Interface browser contract server did not become ready" >&2
  exit 1
fi

chrome_args=(
  --headless=new
  --disable-background-networking
  --disable-component-update
  --disable-default-apps
  --disable-dev-shm-usage
  --disable-extensions
  --disable-sync
  --metrics-recording-only
  --no-first-run
  --no-default-browser-check
  --host-resolver-rules="MAP * ~NOTFOUND, EXCLUDE 127.0.0.1"
  --remote-debugging-address=127.0.0.1
  --remote-debugging-port="$debug_port"
  --user-data-dir="$work_dir/chrome-profile"
)
if [[ $(id -u) -eq 0 ]]; then
  chrome_args+=(--no-sandbox)
fi

setsid "$chrome_bin" "${chrome_args[@]}" about:blank \
  >"$chrome_stdout" 2>"$chrome_log" &
chrome_pid=$!

for _ in $(seq 1 100); do
  if curl --fail --silent \
    "http://127.0.0.1:${debug_port}/json/version" >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$chrome_pid" >/dev/null 2>&1; then
    preserve_failure_evidence
    print_failure_evidence
    echo "Chromium exited before DevTools became ready" >&2
    exit 1
  fi
  sleep 0.1
done

if ! curl --fail --silent --show-error \
  "http://127.0.0.1:${debug_port}/json/version" >/dev/null; then
  preserve_failure_evidence
  print_failure_evidence
  echo "Chromium DevTools endpoint did not become ready" >&2
  exit 1
fi

set +e
timeout --signal=TERM --kill-after=5s "${wall_timeout}s" \
  node "$contract_dir/driver.mjs" \
  "http://127.0.0.1:${debug_port}" \
  "http://127.0.0.1:${port}/tests/interface-wasm-browser/index.html" \
  "$dom" \
  "$result_json" \
  >"$driver_log" 2>&1
driver_status=$?
set -e

if [[ "$driver_status" -eq 124 || "$driver_status" -eq 137 ]]; then
  preserve_failure_evidence
  echo "Canonical interface Chromium contract exceeded ${wall_timeout}s wall-clock limit" >&2
  print_failure_evidence
  exit 1
fi

if [[ "$driver_status" -ne 0 ]] || ! grep -q 'data-status="pass"' "$dom"; then
  preserve_failure_evidence
  echo "Canonical interface Chromium contract failed (driver exit $driver_status)" >&2
  print_failure_evidence
  exit 1
fi

cat "$driver_log"
cat "$result_json"
