#!/usr/bin/env bash
set -euo pipefail

namespace="${NAMESPACE:-default}"
external_url="${EXTERNAL_WEBHOOK_URL:-}"
clone_image='ghcr.io/oresoftware/gha-clone-server@sha256:44684171d909f96fe216d529bfc14f6f32a11e87c0f339d1877ac20606223c97'
router_image='ghcr.io/oresoftware/gha-executor-router@sha256:59a31a496e5c528f89acb7643b8ced1ea14bc6c15b1d83b22a37f4ba529708e6'

for command in kubectl jq curl; do
  command -v "$command" >/dev/null 2>&1 || {
    printf '%s is required\n' "$command" >&2
    exit 2
  }
done

check_external_secret() {
  local name="$1"
  kubectl -n "$namespace" get externalsecret "$name" -o json |
    jq -e 'any(.status.conditions[]?; .type == "Ready" and .status == "True")' >/dev/null
}

check_secret_keys() {
  local name="$1"
  shift
  local json
  json="$(kubectl -n "$namespace" get secret "$name" -o json)"
  local key
  for key in "$@"; do
    jq -e --arg key "$key" '.data[$key] | type == "string" and length > 0' \
      <<<"$json" >/dev/null
  done
}

env_value() {
  local deployment="$1"
  local container="$2"
  local variable="$3"
  kubectl -n "$namespace" get deployment "$deployment" -o json |
    jq -r --arg container "$container" --arg variable "$variable" '
      .spec.template.spec.containers[]
      | select(.name == $container)
      | .env[]
      | select(.name == $variable)
      | .value // empty
    '
}

check_deployment() {
  local deployment="$1"
  local container="$2"
  local expected_image="$3"
  kubectl -n "$namespace" rollout status "deployment/$deployment" --timeout=180s >/dev/null
  kubectl -n "$namespace" get deployment "$deployment" -o json |
    jq -e --arg container "$container" --arg image "$expected_image" '
      .spec.replicas == 1 and
      (.status.availableReplicas // 0) >= 1 and
      any(.spec.template.spec.containers[]; .name == $container and .image == $image) and
      .spec.template.spec.automountServiceAccountToken == false
    ' >/dev/null
}

cleanup() {
  local pid
  for pid in "${clone_pf_pid:-}" "${router_pf_pid:-}"; do
    if [[ -n "$pid" ]]; then
      kill "$pid" >/dev/null 2>&1 || true
      wait "$pid" 2>/dev/null || true
    fi
  done
  rm -f "${clone_log:-}" "${router_log:-}" "${external_body:-}"
}
trap cleanup EXIT

check_external_secret dd-gha-clone-server-secrets
check_external_secret dd-gha-executor-router-secrets
check_secret_keys dd-gha-clone-server-secrets \
  auth_secret github_webhook_secret github_app_installation_token
check_secret_keys dd-gha-executor-router-secrets inbound_auth
check_secret_keys dd-agent-secrets SERVER_AUTH_SECRET

check_deployment dd-gha-clone-server gha-clone-server "$clone_image"
check_deployment dd-gha-executor-router gha-executor-router "$router_image"

[[ "$(env_value dd-gha-clone-server gha-clone-server GHA_CLONE_EXECUTION_ENABLED)" == true ]]
[[ "$(env_value dd-gha-clone-server gha-clone-server GHA_CLONE_WEBHOOK_EXECUTION_ENABLED)" == true ]]
[[ "$(env_value dd-gha-executor-router gha-executor-router GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED)" == true ]]

clone_log="$(mktemp)"
router_log="$(mktemp)"
kubectl -n "$namespace" port-forward service/dd-gha-clone-server 18125:8125 \
  >"$clone_log" 2>&1 &
clone_pf_pid=$!
kubectl -n "$namespace" port-forward service/dd-gha-executor-router 18126:8126 \
  >"$router_log" 2>&1 &
router_pf_pid=$!

for _ in $(seq 1 30); do
  if curl --fail --silent --show-error http://127.0.0.1:18125/healthz >/dev/null 2>&1 &&
     curl --fail --silent --show-error http://127.0.0.1:18126/healthz >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

curl --fail --silent --show-error http://127.0.0.1:18125/healthz >/dev/null
curl --fail --silent --show-error http://127.0.0.1:18125/readyz >/dev/null
curl --fail --silent --show-error http://127.0.0.1:18126/healthz >/dev/null
curl --fail --silent --show-error http://127.0.0.1:18126/readyz >/dev/null

if [[ -n "$external_url" ]]; then
  case "$external_url" in
    https://*/gha-webhooks/github) ;;
    *)
      printf 'EXTERNAL_WEBHOOK_URL must use HTTPS and end in /gha-webhooks/github\n' >&2
      exit 2
      ;;
  esac
  external_body="$(mktemp)"
  delivery='00000000-0000-4000-8000-000000000001'
  status="$(
    curl --silent --show-error \
      --output "$external_body" \
      --write-out '%{http_code}' \
      --request POST \
      --header 'Content-Type: application/json' \
      --header 'X-GitHub-Event: ping' \
      --header "X-GitHub-Delivery: $delivery" \
      --data '{"repository":{"full_name":"ORESoftware/k8s-cluster"}}' \
      "$external_url"
  )"
  [[ "$status" == 401 ]] || {
    printf 'external route expected application HMAC rejection 401, received %s\n' "$status" >&2
    exit 1
  }
fi

printf 'gha workflow_run fallback is ready in namespace %s\n' "$namespace"
