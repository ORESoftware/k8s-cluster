#!/usr/bin/env bash
set -euo pipefail

endpoint="${1:-${BROWSER_MCP_URL:-https://98.90.186.114/browser-mcp}}"
health_url="${BROWSER_MCP_HEALTH_URL:-${endpoint}/healthz}"
edge_origin="${endpoint%/browser-mcp}"
resource_metadata_url="${edge_origin}/.well-known/oauth-protected-resource/browser-mcp"
server_metadata_url="${edge_origin}/.well-known/oauth-authorization-server/browser-mcp"
operator_secret="${BROWSER_MCP_OAUTH_OPERATOR_SECRET:-}"
content_type='Content-Type: application/json'
accept='Accept: application/json, text/event-stream'

for dependency in curl jq openssl python3; do
  command -v "$dependency" >/dev/null || {
    echo "missing dependency: $dependency" >&2
    exit 2
  }
done

session_id=''
access_token=''

rpc() {
  auth_args=()
  if [[ -n "$access_token" ]]; then
    auth_args=(-H "Authorization: Bearer $access_token")
  fi
  curl --fail-with-body --silent --show-error \
    --connect-timeout 10 \
    --max-time 90 \
    -X POST \
    "${auth_args[@]}" \
    -H "$content_type" \
    -H "$accept" \
    --data-binary "$1" \
    "$endpoint"
}

oauth_login() {
  if [[ -z "$operator_secret" ]]; then
    echo "BROWSER_MCP_OAUTH_OPERATOR_SECRET is required for the full OAuth verifier" >&2
    exit 2
  fi

  echo "checking unauthenticated 401 and OAuth discovery"
  challenge_headers="$(
    curl --silent --show-error \
      --connect-timeout 10 \
      --max-time 20 \
      -D - \
      -o /dev/null \
      -X POST \
      -H "$content_type" \
      -H "$accept" \
      --data-binary '{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"browser-mcp-verifier","version":"1.0"}}}' \
      "$endpoint"
  )"
  grep -qE '^HTTP/[^ ]+ 401' <<<"$challenge_headers"
  grep -qiE '^www-authenticate: Bearer .*resource_metadata=' <<<"$challenge_headers"

  resource_metadata="$(curl --fail-with-body --silent --show-error "$resource_metadata_url")"
  jq -e --arg endpoint "$endpoint" '
    .resource == $endpoint and
    .authorization_servers == [$endpoint] and
    (.scopes_supported | index("mcp:tools")) and
    (.scopes_supported | index("browser:read")) and
    (.scopes_supported | index("browser:act"))
  ' <<<"$resource_metadata" >/dev/null

  server_metadata="$(curl --fail-with-body --silent --show-error "$server_metadata_url")"
  jq -e --arg endpoint "$endpoint" '
    .issuer == $endpoint and
    (.code_challenge_methods_supported | index("S256")) and
    (.grant_types_supported | index("authorization_code")) and
    (.grant_types_supported | index("refresh_token")) and
    (.token_endpoint_auth_methods_supported | index("none")) and
    (.scopes_supported | index("offline_access"))
  ' <<<"$server_metadata" >/dev/null

  registration_endpoint="$(jq -er '.registration_endpoint' <<<"$server_metadata")"
  authorization_endpoint="$(jq -er '.authorization_endpoint' <<<"$server_metadata")"
  token_endpoint="$(jq -er '.token_endpoint' <<<"$server_metadata")"
  redirect_uri='http://127.0.0.1:18181/callback'
  registration="$(
    curl --fail-with-body --silent --show-error \
      -X POST \
      -H "$content_type" \
      --data-binary "$(
        jq -cn --arg redirect_uri "$redirect_uri" '{
          redirect_uris: [$redirect_uri],
          client_name: "browser-mcp-verifier",
          grant_types: ["authorization_code", "refresh_token"],
          response_types: ["code"],
          token_endpoint_auth_method: "none"
        }'
      )" \
      "$registration_endpoint"
  )"
  client_id="$(jq -er '.client_id' <<<"$registration")"

  verifier="$(openssl rand -base64 48 | tr '+/' '-_' | tr -d '=\n')"
  challenge="$(
    printf '%s' "$verifier" |
      openssl dgst -sha256 -binary |
      openssl base64 -A |
      tr '+/' '-_' |
      tr -d '='
  )"
  state="$(openssl rand -hex 16)"
  authorization_page="$(
    curl --fail-with-body --silent --show-error --get \
      --data-urlencode 'response_type=code' \
      --data-urlencode "client_id=$client_id" \
      --data-urlencode "redirect_uri=$redirect_uri" \
      --data-urlencode "code_challenge=$challenge" \
      --data-urlencode 'code_challenge_method=S256' \
      --data-urlencode "resource=$endpoint" \
      --data-urlencode 'scope=mcp:tools browser:read browser:act offline_access' \
      --data-urlencode "state=$state" \
      "$authorization_endpoint"
  )"
  sealed_request="$(
    python3 -c 'import html, re, sys; page=sys.stdin.read(); match=re.search(r"name=\"request\" value=\"([^\"]+)\"", page); assert match; print(html.unescape(match.group(1)))' \
      <<<"$authorization_page"
  )"
  authorization_headers="$(
    curl --fail-with-body --silent --show-error \
      -D - \
      -o /dev/null \
      -X POST \
      --data-urlencode "request=$sealed_request" \
      --data-urlencode "operator_secret=$operator_secret" \
      --data-urlencode 'approve=yes' \
      "$authorization_endpoint"
  )"
  callback="$(
    awk 'tolower($1) == "location:" {sub(/\r$/, "", $2); print $2}' <<<"$authorization_headers"
  )"
  code="$(
    python3 -c 'import sys, urllib.parse; query=urllib.parse.urlparse(sys.argv[1]).query; values=urllib.parse.parse_qs(query); assert values["state"][0] == sys.argv[2]; print(values["code"][0])' \
      "$callback" "$state"
  )"
  token_response="$(
    curl --fail-with-body --silent --show-error \
      -X POST \
      --data-urlencode 'grant_type=authorization_code' \
      --data-urlencode "code=$code" \
      --data-urlencode "redirect_uri=$redirect_uri" \
      --data-urlencode "client_id=$client_id" \
      --data-urlencode "code_verifier=$verifier" \
      --data-urlencode "resource=$endpoint" \
      "$token_endpoint"
  )"
  access_token="$(jq -er '.access_token' <<<"$token_response")"
  jq -e '
    .token_type == "Bearer" and
    .expires_in > 0 and
    (.scope | contains("mcp:tools")) and
    (.scope | contains("browser:read")) and
    (.scope | contains("browser:act")) and
    (.refresh_token | type == "string")
  ' <<<"$token_response" >/dev/null
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

echo "checking public health"
curl --fail-with-body --silent --show-error \
  --connect-timeout 10 \
  --max-time 20 \
  "$health_url" |
  jq -e '.ok == true' >/dev/null

oauth_login

echo "checking authenticated Streamable HTTP negotiation"
sse_status="$(
  curl --silent --show-error \
    --connect-timeout 10 \
    --max-time 20 \
    -o /dev/null \
    -w '%{http_code}' \
    -H "Authorization: Bearer $access_token" \
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
    -H "Authorization: Bearer $access_token" \
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
  ["browser_act", "browser_observe"] and
  all(.result.tools[];
    .securitySchemes[0].type == "oauth2" and
    (.securitySchemes[0].scopes | index("mcp:tools"))
  )
' <<<"$tools" >/dev/null

echo "checking browser_act against an approved CFP domain"
start="$(
  rpc '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"browser_act","arguments":{"intent":"read-only production smoke test","actions":[{"type":"start","initial_url":"https://allthingsopen.org"}]}}}'
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
