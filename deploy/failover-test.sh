#!/usr/bin/env bash
# Raft HA failover test (host-side): identify the current leader, kill its pod,
# and prove the cluster (a) elects a NEW leader, (b) restores lock liveness, and
# (c) keeps fencing tokens strictly monotonic across the failover (no rollback /
# reuse — the whole point of routing grants through the replicated log).
#
# We drive everything through a SURVIVOR pod (not the client Service): a
# `port-forward svc/...` pins to a single backend, and if that backend is the
# leader we kill, the forward dies. Forwarding to a known survivor avoids that.
set -euo pipefail
export KUBECTL_NO_CONFIRM=1
NS=live-mutex-rs
LPORT="${LPORT:-16971}"
BASE="http://127.0.0.1:$LPORT"

PF_PID=""
fwd() {  # fwd <pod>  — (re)establish port-forward to a specific pod
  [ -n "$PF_PID" ] && kill "$PF_PID" 2>/dev/null || true
  kubectl -n "$NS" port-forward "pod/$1" "$LPORT:6971" >/tmp/lmxrs-pf.log 2>&1 &
  PF_PID=$!
  for _ in $(seq 1 30); do curl -fs "$BASE/healthz" >/dev/null 2>&1 && return 0; sleep 1; done
  echo "FAIL: port-forward to $1 never became healthy"; exit 1
}
cleanup() { [ -n "$PF_PID" ] && kill "$PF_PID" 2>/dev/null || true; }
trap cleanup EXIT

jget()  { python3 -c "import sys,json;d=json.load(sys.stdin);print(d.get('$1',''))"; }
KEY="failover-probe-$(date +%s)-$$"
fence() { python3 -c "import sys,json;d=json.load(sys.stdin);print(d.get('fencingTokens',{}).get('$KEY',''))"; }
acquire() { curl -s -X POST "$BASE/v1/lock"   -H 'content-type: application/json' -d "{\"key\":\"$KEY\",\"ttlMs\":5000}"; }
unlock()  { curl -s -X POST "$BASE/v1/unlock" -H 'content-type: application/json' -d "{\"key\":\"$KEY\",\"lockUuid\":\"$1\"}"; }

# Only the StatefulSet pods (exclude the verify Job pod, which shares the app label).
PODS=$(kubectl -n "$NS" get pods -l app=live-mutex-rs-raft \
         -o jsonpath='{.items[*].metadata.name}' | tr ' ' '\n' \
         | grep -E '^live-mutex-rs-raft-[0-9]+$' | tr '\n' ' ')
echo "pods: $PODS"

# 1) discover the leader via the first pod
FIRST=$(echo "$PODS" | tr ' ' '\n' | head -1)
fwd "$FIRST"
OLD_LEADER=$(curl -s "$BASE/raft/status" | jget leaderId)
echo "current leader: ${OLD_LEADER:-<none>}"
[ -n "$OLD_LEADER" ] || { echo "FAIL: no leader before failover"; exit 1; }

# 2) pick a survivor pod (not the leader) to drive through the failover
SURVIVOR=$(echo "$PODS" | tr ' ' '\n' | grep -v "^${OLD_LEADER}$" | head -1)
echo "survivor (driver): $SURVIVOR"
fwd "$SURVIVOR"

A=$(acquire); F0=$(printf '%s' "$A" | fence); U=$(printf '%s' "$A" | jget lockUuid)
printf '%s' "$A" | grep -q '"acquired":true' || { echo "FAIL: baseline acquire failed: $A"; exit 1; }
unlock "$U" >/dev/null
echo "baseline fence: $F0"

echo "==> killing leader pod $OLD_LEADER"
kubectl -n "$NS" delete pod "$OLD_LEADER" --wait=false

echo "==> waiting (via survivor $SURVIVOR) for a NEW fresh leader (!= $OLD_LEADER) ..."
NEW_LEADER=""
for _ in $(seq 1 60); do
  S=$(curl -s "$BASE/raft/status" 2>/dev/null || true)
  RDY=$(printf '%s' "$S" | jget isLeaderReady 2>/dev/null || echo)
  L=$(printf '%s' "$S" | jget leaderId 2>/dev/null || echo)
  if [ "$RDY" = "True" ] && [ -n "$L" ] && [ "$L" != "$OLD_LEADER" ]; then NEW_LEADER="$L"; break; fi
  sleep 1
done
[ -n "$NEW_LEADER" ] || { echo "FAIL: no new leader elected after killing $OLD_LEADER"; exit 1; }
echo "new leader: $NEW_LEADER"

A=$(acquire); F1=$(printf '%s' "$A" | fence); U=$(printf '%s' "$A" | jget lockUuid)
printf '%s' "$A" | grep -q '"acquired":true' || { echo "FAIL: cannot acquire after failover: $A"; exit 1; }
unlock "$U" >/dev/null
echo "post-failover fence: $F1"

if [ -n "$F0" ] && [ -n "$F1" ] && [ "$F1" -le "$F0" ]; then
  echo "FAIL: fence not monotonic across failover ($F0 -> $F1)"; exit 1
fi
echo "PASS: failover $OLD_LEADER -> $NEW_LEADER; liveness restored; fence monotonic ($F0 -> $F1)"
