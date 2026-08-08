#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly SCRIPT_NAME="${0##*/}"
readonly CLONE_IMAGE='ghcr.io/oresoftware/gha-clone-server@sha256:44684171d909f96fe216d529bfc14f6f32a11e87c0f339d1877ac20606223c97'
readonly ROUTER_IMAGE='ghcr.io/oresoftware/gha-executor-router@sha256:59a31a496e5c528f89acb7643b8ced1ea14bc6c15b1d83b22a37f4ba529708e6'
readonly BUILD_SERVER_URL='http://dd-build-server.default.svc.cluster.local:8100'
readonly ROUTER_URL='http://dd-gha-executor-router.default.svc.cluster.local:8126'

namespace="${NAMESPACE:-default}"
probe_live=false
clone_local_port="${CLONE_LOCAL_PORT:-18125}"
router_local_port="${ROUTER_LOCAL_PORT:-18126}"

usage() {
  cat <<'USAGE'
Usage: preflight_gha_clone_webhook.sh [--namespace NAME] [--probe-live]

Read-only preflight for the router-backed signed GitHub webhook lane.

Default mode proves that the clone server and executor router are fully
configured but inert: immutable reviewed images, replicas=0, execution flags
false, split Secret authority, exact repository/workflow rules, AWS-only router
placement, Services, and NetworkPolicies. Secret values are never decoded or
printed.

--probe-live is for a separately reviewed plan-only scale-up. It requires one
available clone-server replica and one available router replica, opens local
port-forwards, and verifies both /healthz and /readyz while every execution flag
remains false. It sends no webhook and performs no Kubernetes write.
USAGE
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

pass() { printf 'PASS  %s\n' "$1"; }
fail() { printf 'FAIL  %s\n' "$1" >&2; exit 1; }

get_json() {
  local kind="$1" name="$2"
  kubectl -n "$namespace" get "$kind" "$name" -o json 2>/dev/null ||
    fail "$kind/$name is unavailable in namespace $namespace"
}

external_secret_ready() {
  local name="$1" target="$2" json
  json="$(get_json externalsecret "$name")"
  jq -e --arg target "$target" '
    .spec.target.name == $target and
    any(.status.conditions[]?; .type == "Ready" and .status == "True")
  ' <<<"$json" >/dev/null || fail "ExternalSecret/$name is not Ready=True for target $target"
  pass "ExternalSecret/$name is Ready=True"
}

secret_has_keys() {
  local name="$1"; shift
  local json key
  json="$(get_json secret "$name")"
  for key in "$@"; do
    jq -e --arg key "$key" '(.data[$key] // "") | type == "string" and length > 0' \
      <<<"$json" >/dev/null || fail "Secret/$name is missing non-empty key $key"
  done
  pass "Secret/$name has the reviewed key set (values were not decoded)"
}

container_json() {
  local deployment_json="$1" container_name="$2"
  jq -ce --arg name "$container_name" '
    .spec.template.spec.containers[]? | select(.name == $name)
  ' <<<"$deployment_json" || fail "Deployment is missing container $container_name"
}

env_literal() {
  local container="$1" name="$2"
  jq -r --arg name "$name" '[.env[]? | select(.name == $name) | .value][0] // empty' <<<"$container"
}

require_secret_binding() {
  local container="$1" env_name="$2" secret_name="$3" secret_key="$4"
  jq -e --arg env_name "$env_name" --arg secret_name "$secret_name" --arg secret_key "$secret_key" '
    any(.env[]?;
      .name == $env_name and
      .valueFrom.secretKeyRef.name == $secret_name and
      .valueFrom.secretKeyRef.key == $secret_key)
  ' <<<"$container" >/dev/null ||
    fail "$env_name is not sourced from Secret/$secret_name key $secret_key"
}

require_config_binding() {
  local container="$1" env_name="$2" config_name="$3" config_key="$4"
  jq -e --arg env_name "$env_name" --arg config_name "$config_name" --arg config_key "$config_key" '
    any(.env[]?;
      .name == $env_name and
      .valueFrom.configMapKeyRef.name == $config_name and
      .valueFrom.configMapKeyRef.key == $config_key)
  ' <<<"$container" >/dev/null ||
    fail "$env_name is not sourced from ConfigMap/$config_name key $config_key"
}

readonly clone_name='dd-gha-clone-server'
readonly router_name='dd-gha-executor-router'
readonly clone_secret='dd-gha-clone-server-secrets'
readonly router_secret='dd-gha-executor-router-secrets'
readonly build_secret='dd-agent-secrets'

external_secret_ready "$clone_secret" "$clone_secret"
secret_has_keys "$clone_secret" auth_secret github_webhook_secret github_app_installation_token
external_secret_ready "$router_secret" "$router_secret"
secret_has_keys "$router_secret" inbound_auth
secret_has_keys "$build_secret" SERVER_AUTH_SECRET

clone_config="$(get_json configmap "$clone_name")"
allowed_csv="$(jq -r '.data.GHA_CLONE_ALLOWED_REPOSITORIES // empty' <<<"$clone_config")"
rules_json="$(jq -r '.data.GHA_CLONE_WORKFLOW_RULES_JSON // empty' <<<"$clone_config")"
[[ -n "$allowed_csv" && -n "$rules_json" ]] || fail 'clone repository allowlist or workflow rules are empty'
allowed_json="$(printf '%s' "$allowed_csv" | tr ',' '\n' | jq -Rsc '
  split("\n") | map(gsub("^[[:space:]]+|[[:space:]]+$"; "")) |
  map(select(length > 0)) | unique
')"
jq -e 'type == "array" and length > 0 and all(.[]; test("^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$"))' \
  <<<"$allowed_json" >/dev/null || fail 'clone repository allowlist contains an invalid owner/name'
jq -e --argjson allowed "$allowed_json" '
  type == "object" and length > 0 and
  (keys as $repos | all($repos[]; . as $repo | $allowed | index($repo) != null)) and
  all(.[];
    type == "array" and length > 0 and
    all(.[];
      type == "string" and startswith(".github/workflows/") and
      (endswith(".yml") or endswith(".yaml")) and (contains("..") | not)))
' <<<"$rules_json" >/dev/null ||
  fail 'clone workflow rules must use allowlisted repositories and bounded workflow paths'
pass 'clone repository allowlist and workflow-path rules are exact and internally consistent'

router_config="$(get_json configmap "$router_name")"
executors_json="$(jq -r '.data.GHA_EXECUTOR_ROUTER_EXECUTORS_JSON // empty' <<<"$router_config")"
[[ -n "$executors_json" ]] || fail 'router executor configuration is empty'
jq -e --arg url "$BUILD_SERVER_URL" '
  type == "array" and length == 2 and
  .[0] == {
    id:"aws-primary", provider:"aws", enabled:true,
    url:$url,
    authPath:"/var/run/secrets/gha-executor-router/aws-build-server-auth"
  } and
  .[1] == {id:"hetzner-secondary", provider:"hetzner", enabled:false}
' <<<"$executors_json" >/dev/null ||
  fail 'router placement must be exact AWS-primary enabled plus credential-free Hetzner-disabled'
pass 'router placement is exact AWS-only with Hetzner inert'

clone_deployment="$(get_json deployment "$clone_name")"
router_deployment="$(get_json deployment "$router_name")"
clone_container="$(container_json "$clone_deployment" 'gha-clone-server')"
router_container="$(container_json "$router_deployment" 'gha-executor-router')"

[[ "$(jq -r '.image' <<<"$clone_container")" == "$CLONE_IMAGE" ]] || fail 'clone server image is not the reviewed digest'
[[ "$(jq -r '.image' <<<"$router_container")" == "$ROUTER_IMAGE" ]] || fail 'executor router image is not the reviewed digest'
[[ "$(env_literal "$clone_container" GHA_CLONE_EXECUTION_ENABLED)" == false ]] || fail 'clone API execution must remain false'
[[ "$(env_literal "$clone_container" GHA_CLONE_WEBHOOK_EXECUTION_ENABLED)" == false ]] || fail 'clone webhook execution must remain false'
[[ "$(env_literal "$clone_container" GHA_CLONE_BUILD_SERVER_URL)" == "$ROUTER_URL" ]] || fail 'clone server must address only the executor router'
[[ "$(env_literal "$router_container" GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED)" == false ]] || fail 'router execution must remain false'
[[ "$(env_literal "$router_container" GHA_EXECUTOR_ROUTER_AUTH_PATH)" == '/var/run/secrets/gha-executor-router/inbound-auth' ]] || fail 'router inbound auth path is unexpected'

require_secret_binding "$clone_container" GHA_CLONE_AUTH_SECRET "$clone_secret" auth_secret
require_secret_binding "$clone_container" GHA_CLONE_GITHUB_WEBHOOK_SECRET "$clone_secret" github_webhook_secret
require_secret_binding "$clone_container" GHA_CLONE_GITHUB_TOKEN "$clone_secret" github_app_installation_token
require_secret_binding "$clone_container" GHA_CLONE_BUILD_SERVER_AUTH "$router_secret" inbound_auth
require_config_binding "$clone_container" GHA_CLONE_ALLOWED_REPOSITORIES "$clone_name" GHA_CLONE_ALLOWED_REPOSITORIES
require_config_binding "$clone_container" GHA_CLONE_WORKFLOW_RULES_JSON "$clone_name" GHA_CLONE_WORKFLOW_RULES_JSON
require_config_binding "$router_container" GHA_EXECUTOR_ROUTER_EXECUTORS_JSON "$router_name" GHA_EXECUTOR_ROUTER_EXECUTORS_JSON

jq -e '
  .spec.template.spec.automountServiceAccountToken == false and
  .spec.template.spec.securityContext.runAsNonRoot == true and
  (.spec.template.spec.containers[] | select(.name == "gha-clone-server") |
    .securityContext.allowPrivilegeEscalation == false and
    .securityContext.readOnlyRootFilesystem == true and
    (.securityContext.capabilities.drop | index("ALL") != null) and
    any(.ports[]?; .name == "http" and .containerPort == 8125))
' <<<"$clone_deployment" >/dev/null || fail 'clone Deployment security/port contract is incomplete'

jq -e '
  .spec.template.spec.automountServiceAccountToken == false and
  .spec.template.spec.securityContext.runAsNonRoot == true and
  (.spec.template.spec.containers[] | select(.name == "gha-executor-router") |
    .securityContext.allowPrivilegeEscalation == false and
    .securityContext.readOnlyRootFilesystem == true and
    (.securityContext.capabilities.drop | index("ALL") != null) and
    any(.ports[]?; .name == "http" and .containerPort == 8126)) and
  any(.spec.template.spec.volumes[]?.projected.sources[]?.secret;
    .name == "dd-gha-executor-router-secrets" and
    any(.items[]?; .key == "inbound_auth" and .path == "inbound-auth")) and
  any(.spec.template.spec.volumes[]?.projected.sources[]?.secret;
    .name == "dd-agent-secrets" and
    any(.items[]?; .key == "SERVER_AUTH_SECRET" and .path == "aws-build-server-auth"))
' <<<"$router_deployment" >/dev/null || fail 'router Deployment security/projected-authority contract is incomplete'
pass 'clone and router Deployments are digest-pinned, tokenless, non-root, read-only, and execution-disabled'

clone_service="$(get_json service "$clone_name")"
router_service="$(get_json service "$router_name")"
jq -e '.spec.type == "ClusterIP" and .spec.selector.app == "dd-gha-clone-server" and any(.spec.ports[]?; .name == "http" and .port == 8125 and .targetPort == "http")' \
  <<<"$clone_service" >/dev/null || fail 'clone Service contract is invalid'
jq -e '.spec.type == "ClusterIP" and .spec.selector.app == "dd-gha-executor-router" and any(.spec.ports[]?; .name == "http" and .port == 8126 and .targetPort == "http")' \
  <<<"$router_service" >/dev/null || fail 'router Service contract is invalid'
pass 'clone and router Services expose only their reviewed ClusterIP ports'

clone_policy="$(get_json networkpolicy "$clone_name")"
router_policy="$(get_json networkpolicy "$router_name")"
jq -e '
  .spec.podSelector.matchLabels.app == "dd-gha-clone-server" and
  (.spec.policyTypes | index("Ingress") != null) and (.spec.policyTypes | index("Egress") != null) and
  any(.spec.ingress[]?.from[]?.podSelector.matchLabels.app; . == "dd-remote-gateway") and
  any(.spec.ingress[]?.ports[]?; .protocol == "TCP" and .port == 8125) and
  any(.spec.egress[]?; any(.to[]?.podSelector.matchLabels.app; . == "dd-gha-executor-router") and any(.ports[]?; .protocol == "TCP" and .port == 8126)) and
  any(.spec.egress[]?; any(.to[]?.ipBlock.cidr; . == "0.0.0.0/0") and any(.ports[]?; .protocol == "TCP" and .port == 443)) and
  ([.spec.egress[]?.to[]?.podSelector.matchLabels.app] | index("dd-build-server") == null)
' <<<"$clone_policy" >/dev/null || fail 'clone NetworkPolicy must allow gateway ingress, router egress, HTTPS GitHub access, and no direct build-server path'

jq -e '
  .spec.podSelector.matchLabels.app == "dd-gha-executor-router" and
  (.spec.policyTypes | index("Ingress") != null) and (.spec.policyTypes | index("Egress") != null) and
  any(.spec.ingress[]?.from[]?.podSelector.matchLabels.app; . == "dd-gha-clone-server") and
  any(.spec.ingress[]?.ports[]?; .protocol == "TCP" and .port == 8126) and
  any(.spec.egress[]?; any(.to[]?.podSelector.matchLabels.app; . == "dd-build-server") and any(.ports[]?; .protocol == "TCP" and .port == 8100)) and
  ([.spec.egress[]?.to[]?.ipBlock.cidr] | length == 0) and
  ([.spec.egress[]?.ports[]? | select(.protocol == "TCP") | .port] | index(443) == null)
' <<<"$router_policy" >/dev/null || fail 'router NetworkPolicy must allow clone ingress and AWS build-server egress with no public/Hetzner path'
pass 'NetworkPolicies enforce gateway -> clone -> router -> AWS build-server only'

clone_replicas="$(jq -r '.spec.replicas // 1' <<<"$clone_deployment")"
router_replicas="$(jq -r '.spec.replicas // 1' <<<"$router_deployment")"
if [[ "$probe_live" != true ]]; then
  [[ "$clone_replicas" == 0 && "$router_replicas" == 0 ]] ||
    fail "static preflight requires clone/router replicas=0, found $clone_replicas/$router_replicas"
  pass 'static read-only preflight completed with both services inert at replicas=0'
  exit 0
fi

[[ "$clone_replicas" == 1 && "$router_replicas" == 1 ]] ||
  fail "--probe-live requires clone/router replicas=1, found $clone_replicas/$router_replicas"
[[ "$(jq -r '.status.availableReplicas // 0' <<<"$clone_deployment")" -ge 1 ]] || fail 'clone server has no available plan-only replica'
[[ "$(jq -r '.status.availableReplicas // 0' <<<"$router_deployment")" -ge 1 ]] || fail 'executor router has no available plan-only replica'

work_dir="$(mktemp -d /tmp/gha-clone-router-preflight.XXXXXX)"
clone_pf=''; router_pf=''
cleanup() {
  for pid in "$clone_pf" "$router_pf"; do
    [[ -n "$pid" ]] || continue
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
  rm -rf "$work_dir"
}
trap cleanup EXIT INT TERM

kubectl -n "$namespace" port-forward "service/$clone_name" "$clone_local_port:8125" --address 127.0.0.1 >"$work_dir/clone-port-forward.log" 2>&1 &
clone_pf=$!
kubectl -n "$namespace" port-forward "service/$router_name" "$router_local_port:8126" --address 127.0.0.1 >"$work_dir/router-port-forward.log" 2>&1 &
router_pf=$!

wait_json() {
  local url="$1" path="$2"
  for _ in $(seq 1 60); do
    if curl --fail --silent --show-error "$url" >"$path" 2>/dev/null; then return 0; fi
    sleep 1
  done
  return 1
}

wait_json "http://127.0.0.1:$clone_local_port/healthz" "$work_dir/clone-health.json" || fail 'clone health endpoint did not become reachable'
wait_json "http://127.0.0.1:$clone_local_port/readyz" "$work_dir/clone-ready.json" || fail 'clone ready endpoint did not become reachable'
wait_json "http://127.0.0.1:$router_local_port/healthz" "$work_dir/router-health.json" || fail 'router health endpoint did not become reachable'
wait_json "http://127.0.0.1:$router_local_port/readyz" "$work_dir/router-ready.json" || fail 'router ready endpoint did not become reachable'

jq -e '
  .ok == true and .executionEnabled == false and .webhookExecutionEnabled == false and
  .authConfigured == true and .webhookConfigured == true and .githubApiConfigured == true and
  .buildServerConfigured == true and .allowedRepositories > 0 and .workflowRules > 0
' "$work_dir/clone-health.json" >/dev/null || fail 'clone health response is not fully configured and plan-only'
jq -e '.ok == true and .executionReady == true' "$work_dir/clone-ready.json" >/dev/null || fail 'clone ready response rejected plan-only mode'
jq -e '
  .ok == true and .executionEnabled == false and .configuredExecutors == 2 and
  .enabledExecutors == 1 and .authConfigured == true
' "$work_dir/router-health.json" >/dev/null || fail 'router health response is not fully configured and plan-only'
jq -e '
  .ok == true and .executionEnabled == false and .executionReady == true and .readyExecutors == []
' "$work_dir/router-ready.json" >/dev/null || fail 'router ready response rejected plan-only mode'
pass 'live clone and router health/readiness passed with every execution gate false'
pass 'read-only router-backed webhook preflight completed; no webhook or Kubernetes write was performed'
