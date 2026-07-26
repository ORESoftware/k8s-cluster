#!/usr/bin/env bash
set -euo pipefail

endpoint="${1:-${BROWSER_MCP_URL:-https://98.90.186.114/browser-mcp}}"
health_url="${BROWSER_MCP_HEALTH_URL:-${endpoint}/healthz}"
content_type='Content-Type: application/json'
accept='Accept: application/json, text/event-stream'

for dependency in curl jq; do
  command -v "$dependency" >/dev/null || {
    echo "missing dependency: $dependency" >&2
    exit 2
  }
done

session_id=''

rpc() {
  curl --fail-with-body --silent --show-error \
    --connect-timeout 10 \
    --max-time 90 \
    -X POST \
    -H "$content_type" \
    -H "$accept" \
    --data-binary "$1" \
    "$endpoint"
}

cleanup() {
  if [[ -z "$session_id" ]]; then
    return
  fi
  cleanup_payload="$(
    jq -cn --arg session_id "$session_id" '{
      jsonrpc: "2.0",
      id: 99,
      method: "tools/call",
      params: {
        name: "browser_act",
        arguments: {
          session_id: $session_id,
          intent: "clean up smoke-test session",
          actions: [{type: "close"}]
        }
      }
    }'
  )"
  rpc "$cleanup_payload" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "checking public health and Streamable HTTP negotiation"
curl --fail-with-body --silent --show-error \
  --connect-timeout 10 \
  --max-time 20 \
  "$health_url" |
  jq -e '.ok == true' >/dev/null

sse_status="$(
  curl --silent --show-error \
    --connect-timeout 10 \
    --max-time 20 \
    -o /dev/null \
    -w '%{http_code}' \
    -H 'Accept: text/event-stream' \
    "$endpoint"
)"
test "$sse_status" = '405'

echo "checking initialize and notifications/initialized"
initialize="$(
  rpc '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"browser-mcp-verifier","version":"1.0"}}}'
)"
jq -e '
  .result.protocolVersion == "2025-11-25" and
  (.result.capabilities.tools | type == "object")
' <<<"$initialize" >/dev/null

initialized_status="$(
  curl --silent --show-error \
    --connect-timeout 10 \
    --max-time 20 \
    -o /dev/null \
    -w '%{http_code}' \
    -X POST \
    -H "$content_type" \
    -H "$accept" \
    --data-binary '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
    "$endpoint"
)"
test "$initialized_status" = '202'

echo "checking tools/list"
tools="$(rpc '{"jsonrpc":"2.0","id":2,"method":"tools/list"}')"
jq -e '
  [.result.tools[].name] | sort ==
  ["browser_act", "browser_observe"]
' <<<"$tools" >/dev/null

echo "checking browser_act against the approved Benefactor domain"
start="$(
  rpc '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"browser_act","arguments":{"intent":"read-only production smoke test","actions":[{"type":"start","initial_url":"https://benefactor.cc"}]}}}'
)"
jq -e '.result.isError == false' <<<"$start" >/dev/null
session_id="$(jq -er '.result.structuredContent.session_id' <<<"$start")"

echo "checking browser_observe"
observe_payload="$(
  jq -cn --arg session_id "$session_id" '{
    jsonrpc: "2.0",
    id: 4,
    method: "tools/call",
    params: {
      name: "browser_observe",
      arguments: {
        session_id: $session_id,
        include: ["summary"],
        max_visible_text_chars: 1000
      }
    }
  }'
)"
observe="$(rpc "$observe_payload")"
jq -e '.result.isError == false' <<<"$observe" >/dev/null

echo "checking that off-allowlist navigation is denied"
blocked_payload="$(
  jq -cn --arg session_id "$session_id" '{
    jsonrpc: "2.0",
    id: 5,
    method: "tools/call",
    params: {
      name: "browser_act",
      arguments: {
        session_id: $session_id,
        intent: "verify the domain ceiling",
        actions: [{type: "goto", url: "https://example.com"}]
      }
    }
  }'
)"
blocked="$(rpc "$blocked_payload")"
jq -e '
  .result.isError == false and
  (.result.structuredContent.status == "blocked" or .result.structuredContent.status == "failed") and
  (
    .result.structuredContent.action_results
    | any(
        (.status == "blocked" or .status == "failed") and
        (.message | contains("not on the allowlist"))
      )
  )
' <<<"$blocked" >/dev/null

echo "closing smoke-test session"
close_payload="$(
  jq -cn --arg session_id "$session_id" '{
    jsonrpc: "2.0",
    id: 6,
    method: "tools/call",
    params: {
      name: "browser_act",
      arguments: {
        session_id: $session_id,
        intent: "close smoke-test session",
        actions: [{type: "close"}]
      }
    }
  }'
)"
rpc "$close_payload" | jq -e '.result.isError == false' >/dev/null
session_id=''

echo "browser MCP end-to-end verification passed: $endpoint"
