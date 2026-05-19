#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
app_manifest="${repo_root}/remote/argocd/apps/dd-gleam-mcp-server.application.yaml"

namespace="${MCP_NAMESPACE:-default}"
argocd_namespace="${ARGOCD_NAMESPACE:-argocd}"
app_name="${MCP_ARGO_APP_NAME:-dd-gleam-mcp-server}"
service_name="${MCP_SERVICE_NAME:-dd-gleam-mcp-server}"
deployment_name="${MCP_DEPLOYMENT_NAME:-dd-gleam-mcp-server}"
service_account="${MCP_SERVICE_ACCOUNT:-system:serviceaccount:${namespace}:dd-gleam-mcp-server}"
expected_app_path="${MCP_EXPECTED_APP_PATH:-remote/gleam-mcp-server/k8s/ec2}"
local_port="${MCP_LOCAL_PORT:-18090}"
rollout_timeout="${MCP_ROLLOUT_TIMEOUT:-300s}"

context="$(kubectl config current-context 2>/dev/null || true)"
if [[ -z "${context}" ]]; then
  echo "No kubectl context is configured. Run this from the EC2 host or a kubeconfig pointed at the EC2 cluster." >&2
  exit 2
fi

if [[ "${ALLOW_NON_EC2_CONTEXT:-false}" != "true" ]]; then
  case "${context}" in
    *minikube*|*kind*|*docker-desktop*|*colima*)
      echo "Refusing to verify EC2 MCP against local kubectl context '${context}'." >&2
      echo "Use the EC2 host kubeconfig, or set ALLOW_NON_EC2_CONTEXT=true only for deliberate local testing." >&2
      exit 2
      ;;
  esac
fi

echo "Using kubectl context: ${context}"
echo "Applying EC2 MCP Argo CD application: ${app_manifest}"
kubectl apply -f "${app_manifest}"

actual_app_path="$(kubectl -n "${argocd_namespace}" get application "${app_name}" -o jsonpath='{.spec.source.path}')"
if [[ "${actual_app_path}" != "${expected_app_path}" ]]; then
  echo "Argo application ${app_name} points at '${actual_app_path}', expected '${expected_app_path}'." >&2
  exit 1
fi

actual_destination_namespace="$(kubectl -n "${argocd_namespace}" get application "${app_name}" -o jsonpath='{.spec.destination.namespace}')"
if [[ "${actual_destination_namespace}" != "${namespace}" ]]; then
  echo "Argo application ${app_name} deploys to namespace '${actual_destination_namespace}', expected '${namespace}'." >&2
  exit 1
fi

if command -v argocd >/dev/null 2>&1 && argocd app get "${app_name}" --grpc-web >/dev/null 2>&1; then
  echo "Syncing ${app_name} with argocd CLI."
  argocd app sync "${app_name}" --grpc-web
  argocd app wait "${app_name}" --health --sync --timeout 300 --grpc-web
else
  echo "argocd CLI not found or not logged in; relying on automated Argo CD sync."
fi

echo "Waiting for MCP deployment rollout."
kubectl -n "${namespace}" rollout status "deployment/${deployment_name}" --timeout="${rollout_timeout}"

require_can_i() {
  local verb="$1"
  local resource="$2"
  local scope_args=("${@:3}")
  if [[ "$(kubectl auth can-i "${verb}" "${resource}" --as="${service_account}" "${scope_args[@]}")" != "yes" ]]; then
    echo "Expected ${service_account} to be able to ${verb} ${resource} ${scope_args[*]}." >&2
    exit 1
  fi
}

require_cannot_i() {
  local verb="$1"
  local resource="$2"
  local scope_args=("${@:3}")
  if [[ "$(kubectl auth can-i "${verb}" "${resource}" --as="${service_account}" "${scope_args[@]}")" != "no" ]]; then
    echo "Expected ${service_account} to be denied ${verb} ${resource} ${scope_args[*]}." >&2
    exit 1
  fi
}

echo "Checking EC2 MCP RBAC."
require_can_i list deployments.apps --all-namespaces
require_can_i list pods --all-namespaces
require_can_i list services --all-namespaces
require_can_i list customresourcedefinitions.apiextensions.k8s.io
require_cannot_i list secrets --all-namespaces
require_cannot_i list configmaps --all-namespaces
require_cannot_i get pods/log -n "${namespace}"
require_cannot_i create pods/exec -n "${namespace}"
require_cannot_i patch deployments.apps -n "${namespace}"

port_forward_log="$(mktemp -t dd-gleam-mcp-port-forward.XXXXXX.log)"
kubectl -n "${namespace}" port-forward "svc/${service_name}" "${local_port}:8090" >"${port_forward_log}" 2>&1 &
port_forward_pid="$!"
cleanup() {
  kill "${port_forward_pid}" >/dev/null 2>&1 || true
  rm -f "${port_forward_log}"
}
trap cleanup EXIT

for _ in {1..40}; do
  if curl -fsS "http://127.0.0.1:${local_port}/healthz" >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "${port_forward_pid}" >/dev/null 2>&1; then
    echo "kubectl port-forward exited early:" >&2
    cat "${port_forward_log}" >&2
    exit 1
  fi
  sleep 0.25
done

echo "Checking MCP tool surface through the EC2 cluster service."
curl -fsS "http://127.0.0.1:${local_port}/mcp" | grep -q '"kubernetes_inventory"'
curl -fsS "http://127.0.0.1:${local_port}/mcp" | grep -q '"human_access_policy"'

curl -fsS \
  -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"human_access_policy","arguments":{}}}' \
  "http://127.0.0.1:${local_port}/mcp" \
  | grep -q '"elevatedMcpToolsEnabled":false'

curl -fsS \
  -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"kubernetes_inventory","arguments":{}}}' \
  "http://127.0.0.1:${local_port}/mcp" \
  | grep -q '"metadataOnlyRequest":true'

echo "EC2 Gleam MCP server verification passed."
