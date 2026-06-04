#!/usr/bin/env bash
set -euo pipefail

repo_root="${REPO_ROOT:-/home/ec2-user/codes/dd/dd-next-1}"
target_revision="${TARGET_REVISION:-dev}"
namespace="${MIP_SOLVER_NAMESPACE:-ai-ml}"
app_name="${MIP_SOLVER_ARGO_APP:-dd-in-house-mip-solver-node}"
master_deployment="${MIP_SOLVER_MASTER_DEPLOYMENT:-dd-in-house-mip-solver-node-master}"
slave_deployment="${MIP_SOLVER_SLAVE_DEPLOYMENT:-dd-in-house-mip-solver-node-slave}"
service_name="${MIP_SOLVER_SERVICE:-dd-in-house-mip-solver-node}"
local_port="${MIP_SOLVER_LOCAL_PORT:-18117}"

cd "${repo_root}"

echo "=== sync EC2 checkout and solver submodule ==="
git fetch origin "${target_revision}"
git merge --ff-only FETCH_HEAD
git submodule sync --recursive
git submodule update --init --recursive \
  remote/deployments/mip-solver-node.rs \
  remote/submodules/discrete-event-system.rs
git rev-parse --short HEAD
git -C remote/deployments/mip-solver-node.rs rev-parse --short HEAD

echo "=== render MIP solver manifests ==="
kubectl kustomize remote/deployments/mip-solver-node.rs/k8s >/tmp/dd-mip-solver-render.yaml
wc -l /tmp/dd-mip-solver-render.yaml

echo "=== apply and force Argo CD sync ==="
pre_sync_phase="$(kubectl -n argocd get "application/${app_name}" -o jsonpath='{.status.operationState.phase}' 2>/dev/null || true)"
pre_sync_started_at="$(kubectl -n argocd get "application/${app_name}" -o jsonpath='{.status.operationState.startedAt}' 2>/dev/null || true)"
pre_sync_finished_at="$(kubectl -n argocd get "application/${app_name}" -o jsonpath='{.status.operationState.finishedAt}' 2>/dev/null || true)"
pre_sync_fingerprint="${pre_sync_phase}|${pre_sync_started_at}|${pre_sync_finished_at}"
kubectl apply -f remote/argocd/apps/dd-in-house-mip-solver-node.application.yaml
kubectl -n argocd annotate "application/${app_name}" \
  argocd.argoproj.io/refresh=hard \
  --overwrite || true
kubectl -n argocd patch "application/${app_name}" --type merge -p \
  '{"operation":{"initiatedBy":{"username":"verify-mip-solver-node"},"info":[{"name":"reason","value":"verify-mip-solver-node"}],"sync":{"revision":"HEAD","prune":true}}}' || true

for attempt in $(seq 1 90); do
  phase="$(kubectl -n argocd get "application/${app_name}" -o jsonpath='{.status.operationState.phase}' 2>/dev/null || true)"
  started_at="$(kubectl -n argocd get "application/${app_name}" -o jsonpath='{.status.operationState.startedAt}' 2>/dev/null || true)"
  finished_at="$(kubectl -n argocd get "application/${app_name}" -o jsonpath='{.status.operationState.finishedAt}' 2>/dev/null || true)"
  sync_status="$(kubectl -n argocd get "application/${app_name}" -o jsonpath='{.status.sync.status}' 2>/dev/null || true)"
  health_status="$(kubectl -n argocd get "application/${app_name}" -o jsonpath='{.status.health.status}' 2>/dev/null || true)"
  revision="$(kubectl -n argocd get "application/${app_name}" -o jsonpath='{.status.sync.revision}' 2>/dev/null || true)"
  operation_fingerprint="${phase}|${started_at}|${finished_at}"
  echo "argo wait ${attempt}/90 phase=${phase:-unknown} sync=${sync_status:-unknown} health=${health_status:-unknown} revision=${revision:-unknown} started=${started_at:-unknown} finished=${finished_at:-unknown}"
  case "${phase}" in
    Succeeded)
      break
      ;;
    Failed|Error)
      if [ "${operation_fingerprint}" = "${pre_sync_fingerprint}" ] && [ "${attempt}" -lt 12 ]; then
        echo "argo wait ${attempt}/90 ignoring stale terminal operation phase while refresh/sync request is accepted"
        sleep 5
        continue
      fi
      kubectl -n argocd get "application/${app_name}" -o yaml | tail -120 || true
      exit 1
      ;;
  esac
  sleep 5
done

echo "=== wait for master/slave rollouts ==="
kubectl -n "${namespace}" rollout status "deployment/${master_deployment}" --timeout=1800s
kubectl -n "${namespace}" rollout status "deployment/${slave_deployment}" --timeout=1800s

echo "=== scale slaves to 3 for distributed smoke ==="
kubectl -n "${namespace}" scale "deployment/${slave_deployment}" --replicas=3
kubectl -n "${namespace}" rollout status "deployment/${slave_deployment}" --timeout=1800s
kubectl -n "${namespace}" wait --for=condition=Ready pod \
  -l app="${master_deployment}" \
  --timeout=1800s
kubectl -n "${namespace}" wait --for=condition=Ready pod \
  -l app="${slave_deployment}" \
  --timeout=1800s
kubectl -n "${namespace}" get deploy,svc,pods,scaledobject \
  -l app.kubernetes.io/name=dd-in-house-mip-solver-node \
  -o wide || true
kubectl -n "${namespace}" get pods \
  -l "app in (${master_deployment},${slave_deployment})" \
  -o wide || true

echo "=== port-forward master service ==="
port_forward_log="/tmp/dd-mip-solver-port-forward.log"
rm -f "${port_forward_log}"
kubectl -n "${namespace}" port-forward "svc/${service_name}" "127.0.0.1:${local_port}:8117" >"${port_forward_log}" 2>&1 &
port_forward_pid="$!"
cleanup() {
  kill "${port_forward_pid}" >/dev/null 2>&1 || true
}
trap cleanup EXIT

for attempt in $(seq 1 120); do
  if curl -fsS "http://127.0.0.1:${local_port}/healthz" >/tmp/dd-mip-solver-health.json 2>/tmp/dd-mip-solver-health.err; then
    break
  fi
  if ! kill -0 "${port_forward_pid}" >/dev/null 2>&1; then
    echo "kubectl port-forward exited early" >&2
    cat "${port_forward_log}" >&2 || true
    exit 1
  fi
  sleep 2
done
cat /tmp/dd-mip-solver-health.json
echo
curl -fsS "http://127.0.0.1:${local_port}/readyz"
echo

echo "=== wait for master to observe 3 slave workers ==="
python3 - <<PY
import json
import sys
import time
import urllib.request

port = "${local_port}"
last = None
for attempt in range(180):
    try:
        with urllib.request.urlopen(
            f"http://127.0.0.1:{port}/mip-solver-cluster/workers",
            timeout=5,
        ) as response:
            body = json.loads(response.read().decode("utf-8"))
    except Exception as error:
        last = {"error": str(error)}
    else:
        workers = body.get("workers") or []
        last = {
            "count": body.get("count", len(workers)),
            "workers": [worker.get("nodeId") for worker in workers],
        }
        if len(workers) >= 3:
            print(json.dumps(last, sort_keys=True))
            print("PROOF remote_mip_solver_master_observed_three_slaves=passed")
            break
    if attempt < 5 or attempt % 15 == 0:
        print(json.dumps({"attempt": attempt + 1, "last": last}, sort_keys=True))
    time.sleep(2)
else:
    print("master did not observe 3 slave workers over NATS", file=sys.stderr)
    print(json.dumps(last, sort_keys=True), file=sys.stderr)
    raise SystemExit(1)
PY

echo "=== generate 100 variable / 200 constraint MIP payload ==="
python3 - <<'PY' >/tmp/dd-mip-solver-100x200.json
import json

n = 100
c = [0.0] * n
c[0] = c[1] = c[2] = 1.0
a = []
b = []
con_names = []

knapsack = [0.0] * n
knapsack[0] = knapsack[1] = knapsack[2] = 2.0
a.append(knapsack)
b.append(5.0)
con_names.append("three_item_capacity")

for var in range(99):
    row = [0.0] * n
    row[var] = 1.0
    a.append(row)
    b.append(1.0)
    con_names.append(f"x{var}_upper")

for var in range(n):
    row = [0.0] * n
    row[var] = -1.0
    a.append(row)
    b.append(0.0)
    con_names.append(f"x{var}_lower")

assert len(a) == 200
payload = {
    "requestId": "remote-100x200-three-slave-smoke",
    "problem": {
        "sense": "max",
        "c": c,
        "a": a,
        "b": b,
        "integerVars": [True] * n,
        "ub": [1.0] * n,
        "varNames": [f"x{i}" for i in range(n)],
        "conNames": con_names,
    },
    "options": {
        "splitDepth": 2,
        "maxSubproblems": 8,
        "maxNodes": 10000,
        "maxTicks": 10000,
        "timeoutMs": 600000,
    },
}
print(json.dumps(payload, separators=(",", ":")))
PY

echo "=== solve remote distributed MIP smoke ==="
python3 - <<PY
import json
import math
import sys
import urllib.request

port = "${local_port}"
with open("/tmp/dd-mip-solver-100x200.json", "rb") as handle:
    payload = handle.read()

request = urllib.request.Request(
    f"http://127.0.0.1:{port}/solve",
    data=payload,
    headers={"content-type": "application/json"},
    method="POST",
)
with urllib.request.urlopen(request, timeout=900) as response:
    body = json.loads(response.read().decode("utf-8"))

print(json.dumps({
    "ok": body.get("ok"),
    "status": body.get("status"),
    "distributed": body.get("distributed"),
    "jobsExpected": body.get("jobsExpected"),
    "jobsPublished": body.get("jobsPublished"),
    "jobsCompleted": body.get("jobsCompleted"),
    "jobsSplit": body.get("jobsSplit"),
    "timedOut": body.get("timedOut"),
    "z": body.get("z"),
    "workersRoute": "/mip-solver-cluster/workers",
}, sort_keys=True))

errors = []
if body.get("ok") is not True:
    errors.append("solve ok was not true")
if body.get("status") != "optimal":
    errors.append(f"status {body.get('status')!r} != optimal")
if body.get("distributed") is not True:
    errors.append("solve did not use distributed NATS path")
if body.get("timedOut") is not False:
    errors.append("solve timed out")
if body.get("jobsExpected") != body.get("jobsCompleted"):
    errors.append("not every expected subproblem completed")
if (body.get("jobsPublished") or 0) < 3:
    errors.append("fewer than 3 jobs were published")
if not math.isclose(float(body.get("z") or 0.0), 2.0, rel_tol=0.0, abs_tol=1e-6):
    errors.append(f"objective {body.get('z')!r} != 2.0")
if len(body.get("x") or []) != 100:
    errors.append("solution vector does not have 100 variables")
if sum(1 for value in (body.get("x") or [])[:3] if float(value) > 0.5) != 2:
    errors.append("expected exactly two selected variables among x0,x1,x2")

if errors:
    print("remote MIP smoke failed:", file=sys.stderr)
    for error in errors:
        print(f"- {error}", file=sys.stderr)
    print(json.dumps(body, sort_keys=True)[:5000], file=sys.stderr)
    raise SystemExit(1)

print("PROOF remote_mip_solver_100x200_three_slave_smoke=passed")
PY

echo "=== master observed workers and solve registry ==="
curl -fsS "http://127.0.0.1:${local_port}/mip-solver-cluster/workers"
echo
curl -fsS "http://127.0.0.1:${local_port}/mip-solver-cluster/solves"
echo
curl -fsS "http://127.0.0.1:${local_port}/metrics" | grep -E 'dd_mip_solver_(subproblem_jobs|workers|solves|active)' || true

echo "=== recent solver pod logs ==="
kubectl -n "${namespace}" logs -l app="${master_deployment}" --all-containers --tail=80 || true
kubectl -n "${namespace}" logs -l app="${slave_deployment}" --all-containers --tail=80 || true
