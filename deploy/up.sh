#!/usr/bin/env bash
# Stand up the BrokerRaft HA cluster on k3d and run the correctness gate + a
# real leader-failover test.
#
#   deploy/up.sh             # create cluster, build, deploy, verify, failover
#   deploy/up.sh --verify    # just re-run the verify Job + failover test
set -euo pipefail
export KUBECTL_NO_CONFIRM=1

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

CLUSTER="live-mutex-rs"
IMAGE="live-mutex-rs-raft:dev"
NS="live-mutex-rs"

run_verify() {
  echo "==> applying verify Job"
  kubectl -n "$NS" delete job lmx-verify --ignore-not-found
  kubectl -n "$NS" delete configmap lmx-verify --ignore-not-found
  kubectl -n "$NS" apply -f deploy/k8s/verify-job.yaml
  echo "==> waiting for verify Job"
  kubectl -n "$NS" wait --for=condition=complete job/lmx-verify --timeout=240s 2>/dev/null &
  cpid=$!
  kubectl -n "$NS" wait --for=condition=failed job/lmx-verify --timeout=240s 2>/dev/null &
  fpid=$!
  wait -n "$cpid" "$fpid" || true
  kubectl -n "$NS" logs job/lmx-verify
  kubectl -n "$NS" get job lmx-verify -o jsonpath='{.status.succeeded}' | grep -q 1 \
    || { echo "==> CORRECTNESS GATE FAILED"; return 1; }
  echo "==> CORRECTNESS GATE PASSED"
  echo "==> running leader-failover test"
  bash deploy/failover-test.sh
}

if [[ "${1:-}" == "--verify" ]]; then run_verify; exit $?; fi

echo "==> ensuring k3d cluster '$CLUSTER'"
k3d cluster list "$CLUSTER" >/dev/null 2>&1 || k3d cluster create --config deploy/k3d/cluster.yaml

echo "==> building $IMAGE from Dockerfile.raft (full Rust build — slow)"
docker build -f Dockerfile.raft -t "$IMAGE" .

echo "==> importing image into k3d"
k3d image import "$IMAGE" -c "$CLUSTER"

echo "==> applying manifests"
kubectl apply -k deploy/k8s

echo "==> waiting for StatefulSet rollout"
kubectl -n "$NS" rollout status statefulset/live-mutex-rs-raft --timeout=240s
kubectl -n "$NS" get pods -o wide

run_verify
