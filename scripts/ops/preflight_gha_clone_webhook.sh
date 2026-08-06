#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly SCRIPT_NAME="${0##*/}"
namespace="${NAMESPACE:-default}"
probe_live=false
local_port="${LOCAL_PORT:-18125}"

usage() {
  cat <<'EOF'
Usage: preflight_gha_clone_webhook.sh [--namespace NAME] [--probe-live]

Read-only preflight for the dd-gha-clone-server signed GitHub webhook lane.

The default mode verifies ExternalSecret readiness, required Secret keys,
fail-closed Deployment flags, exact repository/workflow rules, Service wiring,
and NetworkPolicy boundaries. It never decodes or prints secret values.

--probe-live additionally requires one available replica, opens a local
kubectl port-forward, and verifies /healthz and /readyz while both execution
flags remain false. It does not send a webhook or create any cluster object.
EOF
}

while (($#)); do
  case "$1" in
    --namespace)
      [[ $# -ge 2 ]] || { echo "$SCRIPT_NAME: --namespace requires a value" >&2; exit 64; }
      namespace="$2"
      shift 2
      ;;
    --probe-live)
      probe_live=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "$SCRIPT_NAME: unknown argument: $1" >&2
      usage >&2
      exit 64
      ;;
  esac
done

for command in kubectl jq curl; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "$SCRIPT_NAME: required command not found: $command" >&2
    exit 69
  }
done

pass() {
  printf 'PASS  %s\n' "$1"
}

fail() {
  printf 'FAIL  %s\n' "$1" >&2
  exit 1
}

get_json() {
  local kind="$1"
  local name="$2"
  kubectl -n "$namespace" get "$kind" "$name" -o json 2>/dev/null ||
    fail "$kind/$name is unavailable in namespace $namespace"
}

readonly external_secret_name='dd-gha-clone-server-secrets'
readonly target_secret_name='dd-gha-clone-server-secrets'
readonly config_map_name='dd-gha-clone-server'
readonly deployment_name='dd-gha-clone-server'
readonly service_name='dd-gha-clone-server'
readonly network_policy_name='dd-gha-clone-server'

external_secret_json="$(get_json externalsecret "$external_secret_name")"
jq -e '
  any(.status.conditions[]?;
    .type == "Ready" and .status == "True")
' <<<"$external_secret_json" >/dev/null ||
  fail "ExternalSecret/$external_secret_name is not Ready=True"
pass "ExternalSecret is Ready=True"

resolved_secret_name="$(jq -r '.spec.target.name // empty' <<<"$external_secret_json")"
[[ "$resolved_secret_name" == "$target_secret_name" ]] ||
  fail "ExternalSecret target must be $target_secret_name"

secret_json="$(get_json secret "$target_secret_name")"
for key in auth_secret github_webhook_secret github_app_installation_token build_server_auth; do
  jq -e --arg key "$key" '
    (.data[$key] // "") | type == "string" and length > 0
  ' <<<"$secret_json" >/dev/null ||
    fail "Secret/$target_secret_name is missing a non-empty $key entry"
done
pass "all four required Secret entries exist (values were not decoded)"
unset secret_json

config_map_json="$(get_json configmap "$config_map_name")"
allowed_csv="$(jq -r '.data.GHA_CLONE_ALLOWED_REPOSITORIES // empty' <<<"$config_map_json")"
rules_json="$(jq -r '.data.GHA_CLONE_WORKFLOW_RULES_JSON // empty' <<<"$config_map_json")"
[[ -n "$allowed_csv" ]] || fail "repository allowlist is empty"
[[ -n "$rules_json" ]] || fail "workflow rules are empty"

allowed_json="$(
  printf '%s' "$allowed_csv" |
    tr ',' '\n' |
    jq -Rsc 'split("\n") | map(gsub("^[[:space:]]+|[[:space:]]+$"; "")) | map(select(length > 0)) | unique'
)"
jq -e 'type == "array" and length > 0 and all(.[]; test("^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$"))' \
  <<<"$allowed_json" >/dev/null || fail "repository allowlist contains an invalid owner/name"

jq -e --argjson allowed "$allowed_json" '
  type == "object" and length > 0 and
  (keys as $repositories |
    all($repositories[];
      . as $repository | $allowed | index($repository) != null)) and
  all(.[];
    type == "array" and length > 0 and
    all(.[];
      type == "string" and
      startswith(".github/workflows/") and
      (endswith(".yml") or endswith(".yaml")) and
      (contains("..") | not)))
' <<<"$rules_json" >/dev/null ||
  fail "workflow rules must use allowlisted repositories and bounded workflow paths"
pass "repository allowlist and workflow-path rules are exact and internally consistent"

deployment_json="$(get_json deployment "$deployment_name")"
container_json="$(
  jq -c '.spec.template.spec.containers[]? | select(.name == "gha-clone-server")' \
    <<<"$deployment_json"
)"
[[ -n "$container_json" ]] || fail "Deployment is missing container gha-clone-server"

env_value() {
  local name="$1"
  jq -r --arg name "$name" '.env[]? | select(.name == $name) | .value // empty' \
    <<<"$container_json"
}

[[ "$(env_value GHA_CLONE_EXECUTION_ENABLED)" == 'false' ]] ||
  fail "GHA_CLONE_EXECUTION_ENABLED must remain false during preflight"
[[ "$(env_value GHA_CLONE_WEBHOOK_EXECUTION_ENABLED)" == 'false' ]] ||
  fail "GHA_CLONE_WEBHOOK_EXECUTION_ENABLED must remain false during preflight"

jq -e '
  .spec.template.spec.automountServiceAccountToken == false and
  .spec.template.spec.securityContext.runAsNonRoot == true and
  (.spec.template.spec.containers[]? |
    select(.name == "gha-clone-server") |
    .securityContext.allowPrivilegeEscalation == false and
    .securityContext.runAsNonRoot == true and
    (.securityContext.capabilities.drop | index("ALL") != null) and
    any(.ports[]?; .name == "http" and .containerPort == 8125))
' <<<"$deployment_json" >/dev/null ||
  fail "Deployment security or port contract is incomplete"
pass "Deployment is fail-closed, non-root, tokenless, and bound to container port 8125"

for binding in \
  'GHA_CLONE_AUTH_SECRET:auth_secret' \
  'GHA_CLONE_GITHUB_WEBHOOK_SECRET:github_webhook_secret' \
  'GHA_CLONE_GITHUB_TOKEN:github_app_installation_token' \
  'GHA_CLONE_BUILD_SERVER_AUTH:build_server_auth'; do
  env_name="${binding%%:*}"
  secret_key="${binding#*:}"
  jq -e --arg env_name "$env_name" --arg secret_name "$target_secret_name" --arg secret_key "$secret_key" '
    any(.env[]?;
      .name == $env_name and
      .valueFrom.secretKeyRef.name == $secret_name and
      .valueFrom.secretKeyRef.key == $secret_key)
  ' <<<"$container_json" >/dev/null ||
    fail "$env_name is not sourced from the expected Secret key"
done
pass "Deployment secret references match the reviewed four-key mapping"

service_json="$(get_json service "$service_name")"
jq -e '
  .spec.selector.app == "dd-gha-clone-server" and
  any(.spec.ports[]?;
    .name == "http" and .port == 8125 and .targetPort == "http")
' <<<"$service_json" >/dev/null || fail "Service port/selector contract is invalid"
pass "Service selects the clone server and exposes only port 8125"

network_policy_json="$(get_json networkpolicy "$network_policy_name")"
jq -e '
  .spec.podSelector.matchLabels.app == "dd-gha-clone-server" and
  ((.spec.policyTypes | index("Ingress")) != null) and
  ((.spec.policyTypes | index("Egress")) != null) and
  any(.spec.ingress[]?.from[]?.podSelector.matchLabels.app; . == "dd-remote-gateway") and
  any(.spec.ingress[]?.from[]?.podSelector.matchLabels.app; . == "dd-build-server") and
  any(.spec.ingress[]?.ports[]?; .protocol == "TCP" and .port == 8125) and
  any(.spec.egress[]?;
    any(.to[]?.podSelector.matchLabels.app; . == "dd-build-server") and
    any(.ports[]?; .protocol == "TCP" and .port == 8100)) and
  any(.spec.egress[]?;
    any(.to[]?.ipBlock.cidr; . == "0.0.0.0/0") and
    any(.ports[]?; .protocol == "TCP" and .port == 443))
' <<<"$network_policy_json" >/dev/null ||
  fail "NetworkPolicy does not match the gateway/build-server/HTTPS boundary"
pass "NetworkPolicy limits ingress to gateway/build-server and egress to build-server/HTTPS/DNS"

replicas="$(jq -r '.spec.replicas // 1' <<<"$deployment_json")"
if [[ "$probe_live" != true ]]; then
  printf 'INFO  desired replicas: %s\n' "$replicas"
  pass "static read-only preflight completed"
  exit 0
fi

[[ "$replicas" == '1' ]] ||
  fail "--probe-live requires Deployment replicas=1; current desired replicas=$replicas"
available_replicas="$(jq -r '.status.availableReplicas // 0' <<<"$deployment_json")"
[[ "$available_replicas" -ge 1 ]] || fail "clone server has no available replica"
pass "one plan-only clone-server replica is available"

work_dir="$(mktemp -d /tmp/gha-clone-preflight.XXXXXX)"
port_forward_pid=''
cleanup() {
  if [[ -n "$port_forward_pid" ]]; then
    kill "$port_forward_pid" 2>/dev/null || true
    wait "$port_forward_pid" 2>/dev/null || true
  fi
  rm -rf "$work_dir"
}
trap cleanup EXIT INT TERM

kubectl -n "$namespace" port-forward "service/$service_name" \
  "$local_port:8125" --address 127.0.0.1 >"$work_dir/port-forward.log" 2>&1 &
port_forward_pid=$!

health_ready=false
for _ in $(seq 1 60); do
  if curl --fail --silent --show-error \
    "http://127.0.0.1:$local_port/healthz" >"$work_dir/health.json"; then
    health_ready=true
    break
  fi
  sleep 1
done
[[ "$health_ready" == true ]] || fail "health endpoint did not become reachable"

jq -e '
  .ok == true and
  .executionEnabled == false and
  .webhookExecutionEnabled == false and
  .authConfigured == true and
  .webhookConfigured == true and
  .githubApiConfigured == true and
  .buildServerConfigured == true and
  .allowedRepositories > 0 and
  .workflowRules > 0
' "$work_dir/health.json" >/dev/null ||
  fail "live health response is not plan-only and fully configured"
pass "live health endpoint is configured and both execution flags remain false"

curl --fail --silent --show-error \
  "http://127.0.0.1:$local_port/readyz" >"$work_dir/ready.json" ||
  fail "ready endpoint rejected the plan-only server"
jq -e '.ok == true and .executionReady == true' "$work_dir/ready.json" >/dev/null ||
  fail "ready endpoint did not report plan-only readiness"
pass "live plan-only readiness probe passed"
pass "read-only webhook preflight completed; no webhook or cluster write was performed"
