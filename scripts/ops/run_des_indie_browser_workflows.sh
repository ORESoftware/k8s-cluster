#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly PLAYWRIGHT_REPO='discrete-event-systems-test/des-web-playwright-e2e'
readonly PLAYWRIGHT_SHA='1e1116ef6811c4e3e6be34ad3e1def39bc20ef59'
readonly PLAYWRIGHT_PATH='.github/workflows/gha-indie-worker.yml'
readonly PUPPETEER_REPO='discrete-event-systems-test/des-web-puppeteer-e2e'
readonly PUPPETEER_SHA='0547548429d937023a124de37afca7659a85c3dd'
readonly PUPPETEER_PATH='.github/workflows/gha-indie-worker.yml'
readonly REQUEST_SUFFIX="${DES_REQUEST_SUFFIX:?DES_REQUEST_SUFFIX is required}"
readonly WORK_DIR='/tmp/des-indie-browser-execution'
readonly BRIDGE_AGENT_KEY='chatgpt-des-browser-reconciler'
readonly BRIDGE_QUERY='DES browser automation, gha-indie-worker exact-SHA execution, and dd-build-server policy reconciliation'

kubeconfig=''
for candidate in /etc/kubernetes/admin.conf /root/.kube/config /home/ec2-user/.kube/config; do
  if [[ -r "$candidate" ]]; then
    kubeconfig="$candidate"
    break
  fi
done
[[ -n "$kubeconfig" ]] || {
  echo 'No readable Kubernetes administrator configuration was found.' >&2
  exit 69
}
export KUBECONFIG="$kubeconfig"

server_auth=''
bridge_auth=''
bridge_channel=''
port_forward_pid=''
bridge_forward_pid=''
cleanup() {
  unset server_auth encoded_auth bridge_auth encoded_bridge_auth
  if [[ -n "$port_forward_pid" ]]; then
    kill "$port_forward_pid" 2>/dev/null || true
    wait "$port_forward_pid" 2>/dev/null || true
  fi
  if [[ -n "$bridge_forward_pid" ]]; then
    kill "$bridge_forward_pid" 2>/dev/null || true
    wait "$bridge_forward_pid" 2>/dev/null || true
  fi
  rm -rf "$WORK_DIR"
}
trap cleanup EXIT INT TERM
rm -rf "$WORK_DIR"
mkdir -p "$WORK_DIR"

bridge_post() {
  local path="$1"
  local payload="$2"
  curl --fail --silent --show-error \
    --header "authorization: Bearer $bridge_auth" \
    --header 'content-type: application/json' \
    --data-binary "$payload" \
    "http://127.0.0.1:18142$path"
}

bridge_message() {
  local content="$1"
  [[ -n "$bridge_channel" && -n "$bridge_auth" ]] || return 0
  local payload
  payload="$(jq -nc \
    --arg from "$BRIDGE_AGENT_KEY" \
    --arg content "$content" \
    --arg request_suffix "$REQUEST_SUFFIX" \
    '{from:$from,role:"assistant",content:$content,meta:{workflow:"des-browser-indie",requestSuffix:$request_suffix}}')"
  bridge_post "/channels/$bridge_channel/messages" "$payload" >/dev/null || true
}

coordinate_bridge() {
  encoded_bridge_auth="$(kubectl -n default get secret dd-ai-agent-bridge-secrets -o jsonpath='{.data.inbox_token}' 2>/dev/null || true)"
  if [[ -z "$encoded_bridge_auth" ]]; then
    echo 'AI_AGENT_BRIDGE_COORDINATION=unavailable reason=missing-secret'
    return 0
  fi
  bridge_auth="$(printf '%s' "$encoded_bridge_auth" | base64 --decode 2>/dev/null || true)"
  unset encoded_bridge_auth
  if [[ -z "$bridge_auth" ]]; then
    echo 'AI_AGENT_BRIDGE_COORDINATION=unavailable reason=empty-token'
    return 0
  fi

  kubectl -n default port-forward service/dd-ai-agent-bridge 18142:8142 --address 127.0.0.1 \
    > "$WORK_DIR/bridge-port-forward.log" 2>&1 &
  bridge_forward_pid=$!
  local ready=false
  for _ in $(seq 1 30); do
    if curl --fail --silent http://127.0.0.1:18142/healthz >/dev/null 2>&1; then
      ready=true
      break
    fi
    sleep 1
  done
  if [[ "$ready" != true ]]; then
    echo 'AI_AGENT_BRIDGE_COORDINATION=unavailable reason=port-forward'
    return 0
  fi

  local register_payload resolve_payload resolve_response
  register_payload="$(jq -nc \
    --arg agent_key "$BRIDGE_AGENT_KEY" \
    --arg request_suffix "$REQUEST_SUFFIX" \
    '{agent_key:$agent_key,display_name:"ChatGPT DES browser reconciler",kind:"other",host:"protected-k8s-ssm",meta:{workflow:"des-browser-indie",requestSuffix:$request_suffix}}')"
  if ! bridge_post '/agents/register' "$register_payload" > "$WORK_DIR/bridge-register.json"; then
    echo 'AI_AGENT_BRIDGE_COORDINATION=unavailable reason=register'
    return 0
  fi

  resolve_payload="$(jq -nc \
    --arg query "$BRIDGE_QUERY" \
    --arg created_by "$BRIDGE_AGENT_KEY" \
    '{query:$query,created_by:$created_by}')"
  if ! resolve_response="$(bridge_post '/channels/resolve' "$resolve_payload")"; then
    echo 'AI_AGENT_BRIDGE_COORDINATION=unavailable reason=resolve'
    return 0
  fi
  printf '%s' "$resolve_response" > "$WORK_DIR/bridge-resolve.json"
  bridge_channel="$(jq -er '.channel.slug' "$WORK_DIR/bridge-resolve.json" 2>/dev/null || true)"
  if [[ -z "$bridge_channel" ]]; then
    echo 'AI_AGENT_BRIDGE_COORDINATION=unavailable reason=no-channel'
    return 0
  fi

  bridge_message "Claiming the DES browser continuity lane for this run. Scope: scripts/ops/run_des_indie_browser_workflows.sh, the dd-build-server GHA continuity policy, and the immutable Playwright/Puppeteer test revisions. Current fix switches planner input from rejected .gha paths to the reviewed .github/workflows/gha-indie-worker.yml files. Please avoid overlapping edits and post blockers in this channel."
  echo "AI_AGENT_BRIDGE_COORDINATION=active channel=$bridge_channel"
}

http_post_json() {
  local url="$1"
  local request_file="$2"
  local response_file="$3"
  local code rc
  set +e
  code="$(curl --silent --show-error \
    --output "$response_file" \
    --write-out '%{http_code}' \
    --header "x-server-auth: $server_auth" \
    --header 'content-type: application/json' \
    --data-binary "@$request_file" \
    "$url")"
  rc=$?
  set -e
  if (( rc != 0 )); then
    bridge_message "Blocked: GHA API transport failed with rc=$rc for $url."
    echo "HTTP transport failed rc=$rc url=$url" >&2
    cat "$response_file" >&2 2>/dev/null || true
    return "$rc"
  fi
  if [[ ! "$code" =~ ^2[0-9][0-9]$ ]]; then
    bridge_message "Blocked: GHA API returned HTTP $code for $url; inspect the canonical workflow evidence for the bounded response body."
    echo "HTTP $code from $url" >&2
    cat "$response_file" >&2 2>/dev/null || true
    return 22
  fi
}

coordinate_bridge

policy_ready=false
for _ in $(seq 1 240); do
  env_json="$(kubectl -n default get deployment dd-build-server -o json 2>/dev/null || true)"
  if jq -e --arg p "$PLAYWRIGHT_REPO" --arg u "$PUPPETEER_REPO" '
    [.spec.template.spec.containers[] | select(.name == "build-server") | .env[]?]
    | (any(.name == "BUILD_SERVER_ALLOWED_PROFILE_REPO_PREFIXES" and (.value | contains($p) and contains($u))))
      and (any(.name == "BUILD_SERVER_GHA_WORKFLOW_EXECUTION_ENABLED" and .value == "true"))
  ' <<<"$env_json" >/dev/null 2>&1; then
    policy_ready=true
    break
  fi
  sleep 3
done
[[ "$policy_ready" == true ]] || {
  bridge_message 'Blocked: the deployed dd-build-server did not reconcile the DES execution policy before timeout.'
  echo 'The deployed build server did not reconcile the DES execution policy.' >&2
  exit 70
}
kubectl -n default rollout status deployment/dd-build-server --timeout=600s >/dev/null

encoded_auth="$(kubectl -n default get secret dd-agent-secrets -o jsonpath='{.data.SERVER_AUTH_SECRET}')"
[[ -n "$encoded_auth" ]]
server_auth="$(printf '%s' "$encoded_auth" | base64 --decode)"
unset encoded_auth
[[ -n "$server_auth" ]]

kubectl -n default port-forward service/dd-build-server 18100:8100 --address 127.0.0.1 \
  > "$WORK_DIR/port-forward.log" 2>&1 &
port_forward_pid=$!
ready=false
for _ in $(seq 1 90); do
  if curl --fail --silent --header "x-server-auth: $server_auth" \
    http://127.0.0.1:18100/gha/workflows/capabilities \
    > "$WORK_DIR/capabilities.json"; then
    ready=true
    break
  fi
  sleep 1
done
[[ "$ready" == true ]]
jq -e '
  .executionEnabled == true and
  (.profiles | index("playwright") != null) and
  (.profiles | index("puppeteer") != null)
' "$WORK_DIR/capabilities.json" >/dev/null

run_one() {
  local repository="$1"
  local revision="$2"
  local workflow_path="$3"
  local expected_profile="$4"
  local slug="$5"
  local run_id status workflow_url

  [[ "$workflow_path" == .github/workflows/*.yml || "$workflow_path" == .github/workflows/*.yaml ]]
  workflow_url="https://raw.githubusercontent.com/$repository/$revision/$workflow_path"
  curl --fail --silent --show-error --location \
    --output "$WORK_DIR/$slug-workflow.yml" \
    "$workflow_url"

  jq -nc \
    --arg repository "$repository" \
    --arg revision "$revision" \
    --arg workflowPath "$workflow_path" \
    --rawfile workflowYaml "$WORK_DIR/$slug-workflow.yml" \
    --arg requestId "des-browser-$slug-$revision-$REQUEST_SUFFIX" \
    '{
      schemaVersion:"gha-indie-workflow.v1",
      repository:$repository,
      revision:$revision,
      workflowPath:$workflowPath,
      workflowYaml:$workflowYaml,
      requestId:$requestId
    }' > "$WORK_DIR/$slug-request.json"

  bridge_message "Executing $slug at $repository@$revision using $workflow_path."
  http_post_json \
    'http://127.0.0.1:18100/gha/workflows/plan' \
    "$WORK_DIR/$slug-request.json" \
    "$WORK_DIR/$slug-plan.json"
  jq -e --arg repository "$repository" --arg revision "$revision" --arg profile "$expected_profile" '
    .schemaVersion == "gha-indie-plan.v1" and
    .repository == $repository and
    .revision == $revision and
    .immutableRevision == true and
    .executable == true and
    (.jobs | length == 1) and
    .jobs[0].supported == true and
    .jobs[0].profile == $profile
  ' "$WORK_DIR/$slug-plan.json" >/dev/null

  http_post_json \
    'http://127.0.0.1:18100/gha/workflows/runs' \
    "$WORK_DIR/$slug-request.json" \
    "$WORK_DIR/$slug-run.json"
  run_id="$(jq -er '.id' "$WORK_DIR/$slug-run.json")"
  [[ "$run_id" =~ ^gha-[A-Za-z0-9._-]+$ ]]

  status=queued
  for _ in $(seq 1 960); do
    curl --fail --silent --show-error \
      --header "x-server-auth: $server_auth" \
      "http://127.0.0.1:18100/gha/workflows/runs/$run_id" \
      > "$WORK_DIR/$slug-final.json"
    status="$(jq -r '.status' "$WORK_DIR/$slug-final.json")"
    case "$status" in
      succeeded|failed) break ;;
    esac
    sleep 3
  done

  jq -e --arg repository "$repository" --arg revision "$revision" '
    .status == "succeeded" and
    .plan.repository == $repository and
    .plan.revision == $revision and
    (.jobs | length == 1) and
    (all(.jobs[]; .status == "succeeded" and (.buildId | type == "string" and length > 0)))
  ' "$WORK_DIR/$slug-final.json" >/dev/null
  bridge_message "Succeeded: $slug completed through gha-indie-worker at $repository@$revision."
  jq -c '{
    id,
    requestId,
    status,
    repository:.plan.repository,
    revision:.plan.revision,
    workflowPath:.plan.workflowPath,
    profile:.jobs[0].profile,
    buildId:.jobs[0].buildId,
    createdAtMs,
    startedAtMs,
    finishedAtMs
  }' "$WORK_DIR/$slug-final.json"
}

echo 'DES_INDIE_EVIDENCE_BEGIN'
run_one "$PLAYWRIGHT_REPO" "$PLAYWRIGHT_SHA" "$PLAYWRIGHT_PATH" playwright playwright
run_one "$PUPPETEER_REPO" "$PUPPETEER_SHA" "$PUPPETEER_PATH" puppeteer puppeteer
echo 'DES_INDIE_EVIDENCE_END'
bridge_message 'Completed: both exact-SHA DES browser workflows succeeded through gha-indie-worker. GitHub Actions remains the independent hosted lane; evidence is retained by the canonical k8s workflow.'
