#!/usr/bin/env bash
set -euo pipefail
umask 077

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BRIDGE_DIR="${BRIDGE_DIR:-${ROOT}/.e2e/bridge}"
LOG_DIR="${SLACK_SECURITY_LOG_DIR:-${RUNNER_TEMP:-/tmp}/slack-command-security-logs}"
RESULT_DIR="${PLAYWRIGHT_OUTPUT_DIR:-${RUNNER_TEMP:-/tmp}/slack-command-security-results}"
STATE_DIR="${SLACK_COMMAND_STATE_DIR:-${RUNNER_TEMP:-/tmp}/slack-command-security-state}"
REGISTRY_PATH="${SLACK_PROJECT_REGISTRY_PATH:-${ROOT}/remote/tests/fixtures/slack-command-registry.json}"
SIGNING_TEST_SECRET="${SIGNING_TEST_SECRET:-integration-signing-secret}"
SLACK_TEST_TOKEN="${SLACK_TEST_TOKEN:-slack-browser-security-token}"
EXPECTED_SLACK_APP_ID="${EXPECTED_SLACK_APP_ID:-A0BMBAMM5NJ}"
EXPECTED_SLACK_TEAM_ID="${EXPECTED_SLACK_TEAM_ID:-T01B3C83PMK}"
EXPECTED_SLACK_CHANNEL_ID="${EXPECTED_SLACK_CHANNEL_ID:-C0BKP2N3LG7}"
EXPECTED_SLACK_USER_ID="${EXPECTED_SLACK_USER_ID:-U01AZNU2LJ2}"

for required_command in cargo curl jq node; do
  if ! command -v "$required_command" >/dev/null 2>&1; then
    echo "missing required command: ${required_command}" >&2
    exit 1
  fi
done

if [[ ! -f "${BRIDGE_DIR}/Cargo.toml" ]]; then
  echo "missing bridge checkout at ${BRIDGE_DIR}" >&2
  exit 1
fi
if [[ ! -f "$REGISTRY_PATH" ]]; then
  echo "missing Slack registry fixture at ${REGISTRY_PATH}" >&2
  exit 1
fi

mkdir -p "$LOG_DIR" "$RESULT_DIR" "$STATE_DIR"
chmod 700 "$LOG_DIR" "$RESULT_DIR" "$STATE_DIR"

pids=()
last_pid=''
cleanup() {
  local status=$?
  local pid
  trap - EXIT INT TERM
  for pid in "${pids[@]:-}"; do
    kill "$pid" 2>/dev/null || true
  done
  for pid in "${pids[@]:-}"; do
    wait "$pid" 2>/dev/null || true
  done
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

start_process() {
  local name="$1"
  shift
  "$@" >"${LOG_DIR}/${name}.log" 2>&1 &
  local pid=$!
  pids+=("$pid")
  last_pid="$pid"
  printf '%s\n' "$pid" >"${LOG_DIR}/${name}.pid"
  sleep 0.1
  if ! kill -0 "$pid" 2>/dev/null; then
    echo "${name} exited during startup" >&2
    cat "${LOG_DIR}/${name}.log" >&2 || true
    return 1
  fi
}

wait_http() {
  local url="$1"
  local pid="$2"
  local name="$3"
  local attempt
  for attempt in $(seq 1 60); do
    if ! kill -0 "$pid" 2>/dev/null; then
      echo "${name} exited before becoming ready at ${url}" >&2
      cat "${LOG_DIR}/${name}.log" >&2 || true
      return 1
    fi
    if curl \
      --silent \
      --show-error \
      --fail \
      --connect-timeout 1 \
      --max-time 2 \
      "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  echo "timed out waiting for ${name} at ${url}" >&2
  cat "${LOG_DIR}/${name}.log" >&2 || true
  return 1
}

cargo build \
  --manifest-path "${BRIDGE_DIR}/Cargo.toml" \
  --locked \
  --bin fiducia-slack-command

start_process slack-mock \
  env SLACK_MOCK_PORT=8170 \
  node "${ROOT}/remote/tests/fixtures/slack-command-mock.mjs"
slack_mock_pid="$last_pid"
wait_http http://127.0.0.1:8170/healthz "$slack_mock_pid" slack-mock

start_process slack-command \
  env \
    SLACK_COMMAND_HOST=127.0.0.1 \
    SLACK_COMMAND_PORT=8151 \
    SLACK_SIGNING_SECRET="$SIGNING_TEST_SECRET" \
    SLACK_BOT_TOKEN="$SLACK_TEST_TOKEN" \
    SLACK_EXPECTED_APP_ID="$EXPECTED_SLACK_APP_ID" \
    SLACK_EXPECTED_TEAM_ID="$EXPECTED_SLACK_TEAM_ID" \
    SLACK_PROJECT_REGISTRY_PATH="$REGISTRY_PATH" \
    SLACK_COMMAND_STATE_DIR="$STATE_DIR" \
    SLACK_BRIDGE_URL=http://127.0.0.1:8142/ \
    SLACK_COORDINATOR_URL=http://127.0.0.1:8160/ \
    SLACK_API_BASE_URL=http://127.0.0.1:8170/api/ \
    SLACK_CLAUDE_AGENT_KEY=claude-fable-5 \
    SLACK_CHATGPT_AGENT_KEY=gpt-5.6-sol \
    SLACK_LINEAR_RUN_PROJECT_ID=72e891e2-603d-4903-8d08-bd06d204520f \
    SLACK_CONTEXT_MESSAGE_COUNT=5 \
    SLACK_COMMAND_MAX_CONCURRENT_RUNS=1 \
    SLACK_COMMAND_DRY_RUN=true \
    RUST_LOG=info \
  "${BRIDGE_DIR}/target/debug/fiducia-slack-command"
slack_command_pid="$last_pid"
wait_http http://127.0.0.1:8151/readyz "$slack_command_pid" slack-command

curl \
  --silent \
  --show-error \
  --fail \
  --connect-timeout 1 \
  --max-time 2 \
  http://127.0.0.1:8151/readyz \
  | tee "$RESULT_DIR/readiness.json" \
  | jq --exit-status '
      .ok == true and
      .dry_run == true and
      .installed_app_identity_enforced == true
    ' >/dev/null

export PLAYWRIGHT_OUTPUT_DIR="$RESULT_DIR"
export SLACK_COMMAND_BASE_URL=http://127.0.0.1:8151
export SLACK_MOCK_BASE_URL=http://127.0.0.1:8170
export SLACK_SIGNING_SECRET="$SIGNING_TEST_SECRET"
export SLACK_EXPECTED_APP_ID="$EXPECTED_SLACK_APP_ID"
export SLACK_EXPECTED_TEAM_ID="$EXPECTED_SLACK_TEAM_ID"
export SLACK_EXPECTED_CHANNEL_ID="$EXPECTED_SLACK_CHANNEL_ID"
export SLACK_EXPECTED_USER_ID="$EXPECTED_SLACK_USER_ID"
export SLACK_EXPECTED_DRY_RUN=true

cd "${ROOT}/remote/tests"
node --test ui/slack-agent-command-security.playwright.test.mjs 2>&1 \
  | tee "$RESULT_DIR/playwright-security.tap"

if find "$STATE_DIR" -mindepth 1 -maxdepth 1 -print -quit | grep -q .; then
  echo "rejected requests unexpectedly created a durable run claim" >&2
  find "$STATE_DIR" -mindepth 1 -maxdepth 1 -printf '%f\n' >&2
  exit 1
fi

jq -n \
  --arg harness_commit "${GITHUB_SHA:-local}" \
  --arg bridge_commit "$(git -C "$BRIDGE_DIR" rev-parse HEAD)" \
  --arg test_file "remote/tests/ui/slack-agent-command-security.playwright.test.mjs" \
  '{
    schema_version: 1,
    harness_commit: $harness_commit,
    bridge_commit: $bridge_commit,
    test_file: $test_file,
    live_external_credentials: false,
    rejected_run_claim_count: 0
  }' >"$RESULT_DIR/evidence.json"
