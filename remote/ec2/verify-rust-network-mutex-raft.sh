#!/usr/bin/env bash
set -euo pipefail

repo_root="${REPO_ROOT:-/home/ec2-user/codes/dd/dd-next-1}"
namespace="${LMX_RAFT_NAMESPACE:-default}"
app_name="${LMX_RAFT_APP:-dd-next-runtime}"
statefulset="${LMX_RAFT_STATEFULSET:-dd-rust-network-mutex-raft}"
service="${LMX_RAFT_SERVICE:-dd-rust-network-mutex-raft}"
local_port="${LMX_RAFT_LOCAL_PORT:-16971}"
revision="${TARGET_REVISION:-dev}"

cd "$repo_root"

echo "=== sync EC2 checkout ==="
git fetch origin "$revision"
git merge --ff-only FETCH_HEAD
git rev-parse HEAD

echo "=== render dd-next-runtime manifests ==="
kubectl kustomize remote/argocd/dd-next-runtime >/tmp/dd-next-runtime-raft-render.yaml
rg -n "dd-rust-network-mutex-raft|kind: StatefulSet|replicas: 3" /tmp/dd-next-runtime-raft-render.yaml || true

echo "=== apply dd-next-runtime overlay and refresh Argo CD ==="
kubectl apply -k remote/argocd/dd-next-runtime
if kubectl -n argocd get "application/${app_name}" >/dev/null 2>&1; then
  kubectl -n argocd annotate "application/${app_name}" \
    argocd.argoproj.io/refresh=hard --overwrite || true
  kubectl -n argocd patch "application/${app_name}" --type merge -p \
    '{"operation":{"initiatedBy":{"username":"raft-verifier"},"info":[{"name":"reason","value":"verify-rust-network-mutex-raft"}],"sync":{"revision":"HEAD","prune":true}}}' || true
fi

echo "=== wait for Raft StatefulSet rollout ==="
kubectl -n "$namespace" rollout status "statefulset/${statefulset}" --timeout=25m
kubectl -n "$namespace" get statefulset,svc,endpoints,pod -l app=dd-rust-network-mutex-raft -o wide

start_port_forward() {
  local log_file=/tmp/dd-rust-network-mutex-raft-port-forward.log
  rm -f "$log_file"
  kubectl -n "$namespace" port-forward "svc/${service}" "${local_port}:6971" >"$log_file" 2>&1 &
  pf_pid=$!
  for attempt in {1..60}; do
    if curl -fsS "http://127.0.0.1:${local_port}/healthz" >/dev/null 2>&1; then
      return 0
    fi
    if ! kill -0 "$pf_pid" >/dev/null 2>&1; then
      cat "$log_file" >&2 || true
      return 1
    fi
    sleep 1
  done
  cat "$log_file" >&2 || true
  return 1
}

cleanup() {
  if [[ -n "${pf_pid:-}" ]]; then
    kill "$pf_pid" >/dev/null 2>&1 || true
    wait "$pf_pid" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

echo "=== port-forward Raft service ==="
start_port_forward

curl_json() {
  local method="$1"
  local path="$2"
  local body="${3:-}"
  if [[ -n "$body" ]]; then
    curl -fsS -X "$method" "http://127.0.0.1:${local_port}${path}" \
      -H 'content-type: application/json' \
      --data "$body"
  else
    curl -fsS -X "$method" "http://127.0.0.1:${local_port}${path}"
  fi
}

wait_for_leader() {
  local forbidden_leader="${1:-}"
  for attempt in {1..120}; do
    status_json="$(curl_json GET /raft/status || true)"
    leader_id="$(jq -r '.leaderId // empty' <<<"$status_json" 2>/dev/null || true)"
    cluster_size="$(jq -r '.clusterSize // empty' <<<"$status_json" 2>/dev/null || true)"
    quorum_size="$(jq -r '.quorumSize // empty' <<<"$status_json" 2>/dev/null || true)"
    if [[ "$cluster_size" == "3" && "$quorum_size" == "2" && -n "$leader_id" && "$leader_id" != "$forbidden_leader" ]]; then
      echo "$status_json"
      return 0
    fi
    echo "waiting for leader attempt=$attempt leader=${leader_id:-none} forbidden=${forbidden_leader:-none} status=${status_json:-none}"
    sleep 2
  done
  return 1
}

echo "=== wait for initial Raft leader ==="
initial_status="$(wait_for_leader)"
echo "$initial_status" | jq .
leader_id="$(jq -r '.leaderId' <<<"$initial_status")"

echo "=== acquire/release through ClusterIP service ==="
key="lmx-raft-verify-$(date +%s)-$RANDOM"
acquire_json="$(curl_json POST /v1/lock "{\"key\":\"${key}\",\"ttlMs\":5000}")"
echo "$acquire_json" | jq .
if [[ "$(jq -r '.acquired' <<<"$acquire_json")" != "true" ]]; then
  echo "acquire failed" >&2
  exit 1
fi
lock_uuid="$(jq -r '.lockUuid' <<<"$acquire_json")"
release_json="$(curl_json POST /v1/unlock "{\"key\":\"${key}\",\"lockUuid\":\"${lock_uuid}\"}")"
echo "$release_json" | jq .
if [[ "$(jq -r '.unlocked' <<<"$release_json")" != "true" ]]; then
  echo "release failed" >&2
  exit 1
fi

echo "=== fail current leader pod and verify service survives ==="
kubectl -n "$namespace" delete pod "$leader_id" --wait=false
post_failover_status="$(wait_for_leader "$leader_id")"
echo "$post_failover_status" | jq .

key="lmx-raft-failover-$(date +%s)-$RANDOM"
acquire_json="$(curl_json POST /v1/lock "{\"key\":\"${key}\",\"ttlMs\":5000}")"
echo "$acquire_json" | jq .
if [[ "$(jq -r '.acquired' <<<"$acquire_json")" != "true" ]]; then
  echo "post-failover acquire failed" >&2
  exit 1
fi
lock_uuid="$(jq -r '.lockUuid' <<<"$acquire_json")"
release_json="$(curl_json POST /v1/unlock "{\"key\":\"${key}\",\"lockUuid\":\"${lock_uuid}\"}")"
echo "$release_json" | jq .
if [[ "$(jq -r '.unlocked' <<<"$release_json")" != "true" ]]; then
  echo "post-failover release failed" >&2
  exit 1
fi

echo "=== wait for StatefulSet to heal ==="
kubectl -n "$namespace" rollout status "statefulset/${statefulset}" --timeout=25m
kubectl -n "$namespace" get pod,svc,endpoints,statefulset -l app=dd-rust-network-mutex-raft -o wide

echo "PROOF broker_raft_lb_failover_ok service=${service} cluster_size=3 quorum=2 old_leader=${leader_id}"
