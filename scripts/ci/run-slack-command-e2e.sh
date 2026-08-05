#!/usr/bin/env bash
set -euo pipefail
umask 077

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BRIDGE_DIR="${BRIDGE_DIR:-${ROOT}/.e2e/bridge}"
COORDINATOR_DIR="${COORDINATOR_DIR:-${ROOT}/.e2e/coordinator}"
LOG_DIR="${SLACK_E2E_LOG_DIR:-${RUNNER_TEMP:-/tmp}/slack-command-e2e-logs}"
RESULT_DIR="${PLAYWRIGHT_OUTPUT_DIR:-${RUNNER_TEMP:-/tmp}/slack-command-e2e-results}"
STATE_DIR="${SLACK_COMMAND_STATE_DIR:-${RUNNER_TEMP:-/tmp}/slack-command-state}"
BRIDGE_TEST_TOKEN="${BRIDGE_TEST_TOKEN:-bridge-integration-token}"
COORDINATOR_TEST_TOKEN="${COORDINATOR_TEST_TOKEN:-coordinator-integration-token}"
SLACK_TEST_TOKEN="${SLACK_TEST_TOKEN:-slack-integration-token}"
SIGNING_TEST_SECRET="${SIGNING_TEST_SECRET:-integration-signing-secret}"
EXPECTED_SLACK_APP_ID="${EXPECTED_SLACK_APP_ID:-A0BMBAMM5NJ}"
EXPECTED_SLACK_TEAM_ID="${EXPECTED_SLACK_TEAM_ID:-T01B3C83PMK}"
POSTGRES_TEST_IMAGE="${POSTGRES_TEST_IMAGE:-postgres:17.9-bookworm@sha256:47f917f7409eacd22fc5dfb1dee634e1b55cf0c01d1a7eb701be2227a03e0641}"

if [[ "$POSTGRES_TEST_IMAGE" != *@sha256:* ]]; then
  echo "POSTGRES_TEST_IMAGE must be pinned by digest" >&2
  exit 1
fi

for required_command in cargo curl docker jq node; do
  if ! command -v "$required_command" >/dev/null 2>&1; then
    echo "missing required command: ${required_command}" >&2
    exit 1
  fi
done

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
  echo "started ${name} pid=${pid}"
}

bearer_header() {
  local token="$1"
  printf '%s: %s %s' 'Authorization' 'Bearer' "$token"
}

wait_http() {
  local url="$1"
  local bearer="${2:-}"
  local pid="${3:-}"
  local name="${4:-service}"
  local attempt
  for attempt in $(seq 1 180); do
    if [[ -n "$pid" ]] && ! kill -0 "$pid" 2>/dev/null; then
      echo "${name} exited before becoming ready at ${url}" >&2
      cat "${LOG_DIR}/${name}.log" >&2 || true
      return 1
    fi
    if [[ -n "$bearer" ]]; then
      if curl --silent --show-error --fail --connect-timeout 1 --max-time 2 \
        --header "$(bearer_header "$bearer")" "$url" >/dev/null 2>&1; then
        return 0
      fi
    elif curl --silent --show-error --fail --connect-timeout 1 --max-time 2 "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  echo "timed out waiting for ${name} at ${url}" >&2
  cat "${LOG_DIR}/${name}.log" >&2 || true
  return 1
}

if [[ ! -f "${BRIDGE_DIR}/Cargo.toml" ]]; then
  echo "missing bridge checkout at ${BRIDGE_DIR}" >&2
  exit 1
fi
if [[ ! -f "${COORDINATOR_DIR}/Cargo.toml" ]]; then
  echo "missing coordinator checkout at ${COORDINATOR_DIR}" >&2
  exit 1
fi

schema="${COORDINATOR_DIR}/tests/fixtures/ai_agent_coordinator.schema.sql"
test -f "$schema"
docker run --rm \
  --network host \
  --env PGPASSWORD=postgres \
  --volume "${schema}:/schema.sql:ro" \
  "$POSTGRES_TEST_IMAGE" \
  psql \
    --host 127.0.0.1 \
    --username postgres \
    --dbname coordinator \
    --set ON_ERROR_STOP=1 \
    --file /schema.sql

cargo build \
  --manifest-path "${BRIDGE_DIR}/Cargo.toml" \
  --locked \
  --bin fiducia-ai-agent-bridge \
  --bin fiducia-slack-command
cargo build \
  --manifest-path "${COORDINATOR_DIR}/Cargo.toml" \
  --locked \
  --bin ai-agent-coordinator

start_process slack-mock \
  env SLACK_MOCK_PORT=8170 \
  node "${ROOT}/remote/tests/fixtures/slack-command-mock.mjs"
slack_mock_pid="$last_pid"
wait_http http://127.0.0.1:8170/healthz '' "$slack_mock_pid" slack-mock

start_process bridge \
  env \
    HOST=127.0.0.1 \
    HTTP_PORT=8142 \
    TCP_PORT=8143 \
    API_AUTH_BEARER="$BRIDGE_TEST_TOKEN" \
    AI_AGENT_BRIDGE_TOKEN="$BRIDGE_TEST_TOKEN" \
    AI_AGENT_BRIDGE_DIR="${RUNNER_TEMP:-/tmp}/slack-bridge-inbox" \
    LOG_FORMAT=json \
    RUST_LOG=info \
  "${BRIDGE_DIR}/target/debug/fiducia-ai-agent-bridge"
bridge_pid="$last_pid"
wait_http http://127.0.0.1:8142/readyz '' "$bridge_pid" bridge

bridge_auth_header="$(bearer_header "$BRIDGE_TEST_TOKEN")"
for registration in \
  '{"agent_key":"gpt-5.6-sol","display_name":"ChatGPT Sol","kind":"codex","meta":{"capabilities":["rust","github","linear"]}}' \
  '{"agent_key":"claude-fable-5","display_name":"Claude Fable","kind":"claude","meta":{"capabilities":["rust","github","linear"]}}'; do
  agent_key="$(jq --raw-output '.agent_key' <<<"$registration")"
  response_path="${LOG_DIR}/bridge-register-${agent_key}.json"
  status="$(curl --silent --show-error \
    --connect-timeout 2 \
    --max-time 10 \
    --request POST \
    --header "$bridge_auth_header" \
    --header 'Content-Type: application/json' \
    --data "$registration" \
    --output "$response_path" \
    --write-out '%{http_code}' \
    http://127.0.0.1:8142/agents/register)"
  if [[ ! "$status" =~ ^2[0-9][0-9]$ ]] || ! jq --exit-status '.ok == true' "$response_path" >/dev/null; then
    echo "bridge agent registration failed for ${agent_key}: HTTP ${status}" >&2
    cat "$response_path" >&2
    exit 1
  fi
done
unset bridge_auth_header

start_process coordinator \
  env \
    COORDINATOR_CONFIG="${ROOT}/remote/tests/fixtures/slack-command-coordinator.yaml" \
    AI_AGENT_COORDINATOR_DATABASE_URL=postgresql://postgres:postgres@127.0.0.1:5432/coordinator \
    COORDINATOR_API_TOKEN="$COORDINATOR_TEST_TOKEN" \
    GITHUB_REPOSITORY_ADMIN_ENABLED=false \
    LINEAR_DELIVERY_ENABLED=false \
    EMAIL_ATTENTION_ENABLED=false \
    TELEMETRY_AUTOMATION_ENABLED=false \
    RUST_LOG=info \
  "${COORDINATOR_DIR}/target/debug/ai-agent-coordinator"
coordinator_pid="$last_pid"
wait_http http://127.0.0.1:8160/readyz '' "$coordinator_pid" coordinator

start_process slack-command \
  env \
    SLACK_COMMAND_HOST=127.0.0.1 \
    SLACK_COMMAND_PORT=8151 \
    SLACK_SIGNING_SECRET="$SIGNING_TEST_SECRET" \
    SLACK_BOT_TOKEN="$SLACK_TEST_TOKEN" \
    SLACK_EXPECTED_APP_ID="$EXPECTED_SLACK_APP_ID" \
    SLACK_EXPECTED_TEAM_ID="$EXPECTED_SLACK_TEAM_ID" \
    SLACK_PROJECT_REGISTRY_PATH="${ROOT}/remote/tests/fixtures/slack-command-registry.json" \
    SLACK_COMMAND_STATE_DIR="$STATE_DIR" \
    SLACK_BRIDGE_URL=http://127.0.0.1:8142/ \
    SLACK_BRIDGE_BEARER="$BRIDGE_TEST_TOKEN" \
    SLACK_COORDINATOR_URL=http://127.0.0.1:8160/ \
    SLACK_COORDINATOR_BEARER="$COORDINATOR_TEST_TOKEN" \
    SLACK_API_BASE_URL=http://127.0.0.1:8170/api/ \
    SLACK_CLAUDE_AGENT_KEY=claude-fable-5 \
    SLACK_CHATGPT_AGENT_KEY=gpt-5.6-sol \
    SLACK_LINEAR_RUN_PROJECT_ID=72e891e2-603d-4903-8d08-bd06d204520f \
    SLACK_CONTEXT_MESSAGE_COUNT=5 \
    SLACK_COMMAND_MAX_CONCURRENT_RUNS=4 \
    SLACK_COMMAND_DRY_RUN=false \
    RUST_LOG=info \
  "${BRIDGE_DIR}/target/debug/fiducia-slack-command"
slack_command_pid="$last_pid"
wait_http http://127.0.0.1:8151/readyz '' "$slack_command_pid" slack-command

cd "${ROOT}/remote/tests"
PLAYWRIGHT_OUTPUT_DIR="$RESULT_DIR" \
SLACK_COMMAND_BASE_URL=http://127.0.0.1:8151 \
SLACK_MOCK_BASE_URL=http://127.0.0.1:8170 \
BRIDGE_BASE_URL=http://127.0.0.1:8142 \
COORDINATOR_BASE_URL=http://127.0.0.1:8160 \
SLACK_SIGNING_SECRET="$SIGNING_TEST_SECRET" \
SLACK_BRIDGE_BEARER="$BRIDGE_TEST_TOKEN" \
SLACK_COORDINATOR_BEARER="$COORDINATOR_TEST_TOKEN" \
SLACK_EXPECTED_APP_ID="$EXPECTED_SLACK_APP_ID" \
SLACK_EXPECTED_TEAM_ID="$EXPECTED_SLACK_TEAM_ID" \
node --test ui/slack-agent-command.playwright.test.mjs 2>&1 \
  | tee "$RESULT_DIR/playwright.tap"
