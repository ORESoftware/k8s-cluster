#!/usr/bin/env bash
set -euo pipefail

# End-to-end test for the NATS bridge + vapi work-queue path against a REAL
# NATS server (the same image the cluster runs), not a mock.
#
# Covers, in order:
#   1. bridge auth / subject-allowlist / body rejection matrix
#   2. durable subjects fail closed without a stream; explicitly
#      non-durable subjects may use core-NATS fallback
#   3. vapi worker stream+consumer provisioning and task lifecycle
#      (ack-and-drop for poison tasks, bounded NAK redelivery for transient)
#   4. the KEDA scale signal: consumer lag accrues while the worker is down
#      and drains when it returns
#   5. resilience: NATS restart -> bridge /readyz flips 503 and recovers,
#      worker reconnects and resumes consuming
#   6. concurrent publish load and counter integrity
#
# Usage: bash scripts/e2e.sh
# Requires: docker, cargo. Binds localhost ports 14222/18222/13004/18113.

NATS_CONTAINER=vxl-nats-e2e
NATS_PORT=14222
MON_PORT=18222
BRIDGE_PORT=13004
VAPI_PORT=18113
BRIDGE_TOKEN_VALUE="e2e-bridge-token-0123456789"
BRIDGE_URL="http://127.0.0.1:${BRIDGE_PORT}"
MON_URL="http://127.0.0.1:${MON_PORT}"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
BRIDGE_DIR="${REPO_ROOT}/remote/nats-bridge"
VAPI_DIR="${REPO_ROOT}/remote/deployments/rust-vapi-phone-rs"
WORK_DIR="$(mktemp -d)"

PASS=0
FAIL=0
BRIDGE_PID=""
VAPI_PID=""

cleanup() {
  [ -n "$BRIDGE_PID" ] && kill "$BRIDGE_PID" 2>/dev/null || true
  [ -n "$VAPI_PID" ] && kill "$VAPI_PID" 2>/dev/null || true
  docker rm -f "$NATS_CONTAINER" >/dev/null 2>&1 || true
  rm -rf "$WORK_DIR"
}
trap cleanup EXIT

ok()   { PASS=$((PASS+1)); printf '  \033[32mPASS\033[0m %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  \033[31mFAIL\033[0m %s (%s)\n' "$1" "$2"; }
step() { printf '\n\033[1m%s\033[0m\n' "$1"; }

# assert_code <description> <expected-status> <curl args...>
assert_code() {
  local desc="$1" want="$2"; shift 2
  local got
  got="$(curl -s -o /dev/null -w '%{http_code}' "$@" || true)"
  [ "$got" = "$want" ] && ok "$desc" || bad "$desc" "want $want, got $got"
}

# assert_json <description> <jq-ish python expr over stdin json> <curl args...>
assert_json() {
  local desc="$1" expr="$2"; shift 2
  local body
  body="$(curl -s "$@" || true)"
  if python3 -c "
import json,sys
d=json.loads('''$body''')
sys.exit(0 if ($expr) else 1)
" 2>/dev/null; then ok "$desc"; else bad "$desc" "body=$body"; fi
}

publish() {
  local subject="$1" payload="$2"
  curl -s -X POST "${BRIDGE_URL}/publish/${subject}" \
    -H "authorization: Bearer ${BRIDGE_TOKEN_VALUE}" \
    -H 'content-type: application/json' -d "$payload"
}

wait_for() { # wait_for <seconds> <command...>
  local deadline=$(( SECONDS + $1 )); shift
  while [ "$SECONDS" -lt "$deadline" ]; do
    if "$@" >/dev/null 2>&1; then return 0; fi
    sleep 1
  done
  return 1
}

consumer_field() { # consumer_field <field>
  curl -s "${MON_URL}/jsz?consumers=true" | python3 -c "
import json,sys
d=json.load(sys.stdin)
for a in d.get('account_details',[]):
    for s in a.get('stream_detail',[]):
        for c in s.get('consumer_detail',[]):
            if c['name']=='dd-vapi-phone-worker':
                print(c.get('$1', 0)); sys.exit(0)
print('-1')
"
}

start_bridge() {
  BRIDGE_TOKEN="$BRIDGE_TOKEN_VALUE" \
  BRIDGE_SUBJECT_PREFIXES="dd.vapi.tasks.,vxl." \
  BRIDGE_DURABLE_SUBJECT_PREFIXES="dd.vapi.tasks." \
  BRIDGE_MAX_BODY_BYTES=4096 \
  BRIDGE_MAX_IN_FLIGHT=64 \
  BRIDGE_PUBLISH_TIMEOUT_MS=3000 \
  NATS_URL="nats://127.0.0.1:${NATS_PORT}" \
  PORT="$BRIDGE_PORT" \
    "${BRIDGE_DIR}/target/debug/nats-bridge" >"${WORK_DIR}/bridge.log" 2>&1 &
  BRIDGE_PID=$!
}

start_worker() {
  HOST=127.0.0.1 PORT="$VAPI_PORT" \
  VAPI_SERVER_SECRET="e2e-vapi-server-secret-123" \
  VAPI_NATS_URL="nats://127.0.0.1:${NATS_PORT}" \
  VAPI_NATS_ACK_WAIT_SECONDS=5 \
  VAPI_NATS_MAX_DELIVER=3 \
    "${VAPI_DIR}/target/debug/dd-rust-vapi-phone" >>"${WORK_DIR}/vapi.log" 2>&1 &
  VAPI_PID=$!
}

step "Building binaries"
(cd "$BRIDGE_DIR" && cargo build -q)
(cd "$VAPI_DIR" && cargo build -q)

step "Starting NATS (JetStream) in docker"
docker rm -f "$NATS_CONTAINER" >/dev/null 2>&1 || true
# No --rm: step 5 stops and restarts this container to test reconnect.
# The EXIT trap removes it.
docker run -d --name "$NATS_CONTAINER" \
  -p "${NATS_PORT}:4222" -p "${MON_PORT}:8222" \
  nats:2.11.17-alpine -js -m 8222 >/dev/null
wait_for 30 curl -sf "${MON_URL}/healthz" || { echo "NATS never became healthy"; exit 1; }

step "0. Fail-closed startup (the bridge must refuse to be an open relay)"
# Each of these must exit non-zero rather than start serving.
refuses() { # refuses <description> <env assignments...>
  local desc="$1"; shift
  if env "$@" NATS_URL="nats://127.0.0.1:${NATS_PORT}" PORT="$BRIDGE_PORT" \
      "${BRIDGE_DIR}/target/debug/nats-bridge" >/dev/null 2>&1; then
    bad "$desc" "process started anyway"
  else
    ok "$desc"
  fi
}
refuses "refuses to start with no BRIDGE_TOKEN" BRIDGE_SUBJECT_PREFIXES="vxl."
refuses "refuses a BRIDGE_TOKEN under 16 chars" BRIDGE_TOKEN="short" BRIDGE_SUBJECT_PREFIXES="vxl."
refuses "refuses to start with no BRIDGE_SUBJECT_PREFIXES" BRIDGE_TOKEN="$BRIDGE_TOKEN_VALUE"
refuses "refuses an empty BRIDGE_SUBJECT_PREFIXES" BRIDGE_TOKEN="$BRIDGE_TOKEN_VALUE" BRIDGE_SUBJECT_PREFIXES=" , "
refuses "refuses durable prefixes outside the allowlist" BRIDGE_TOKEN="$BRIDGE_TOKEN_VALUE" \
  BRIDGE_SUBJECT_PREFIXES="vxl." BRIDGE_DURABLE_SUBJECT_PREFIXES="dd.vapi.tasks."

start_bridge
wait_for 30 curl -sf "${BRIDGE_URL}/readyz" || { echo "bridge never became ready"; exit 1; }

step "1. Bridge rejection matrix (nothing may reach the bus)"
assert_code "no token -> 401" 401 \
  -X POST "${BRIDGE_URL}/publish/dd.vapi.tasks.call" -H 'content-type: application/json' -d '{}'
assert_code "wrong token -> 401" 401 \
  -X POST "${BRIDGE_URL}/publish/dd.vapi.tasks.call" -H 'authorization: Bearer wrong-token-xxxxxx' \
  -H 'content-type: application/json' -d '{}'
assert_code "off-allowlist subject -> 403" 403 \
  -X POST "${BRIDGE_URL}/publish/dd.remote.contracts.solana.settle" \
  -H "authorization: Bearer ${BRIDGE_TOKEN_VALUE}" -H 'content-type: application/json' -d '{}'
assert_code "\$JS.API stream delete -> 403" 403 \
  -X POST "${BRIDGE_URL}/publish/%24JS.API.STREAM.DELETE.DD_REMOTE_TASKS" \
  -H "authorization: Bearer ${BRIDGE_TOKEN_VALUE}" -H 'content-type: application/json' -d '{}'
assert_code "\$SYS subject -> 403" 403 \
  -X POST "${BRIDGE_URL}/publish/%24SYS.REQ.SERVER.PING" \
  -H "authorization: Bearer ${BRIDGE_TOKEN_VALUE}" -H 'content-type: application/json' -d '{}'
assert_code "wildcard subject -> 403" 403 \
  -X POST "${BRIDGE_URL}/publish/dd.vapi.tasks.%3E" \
  -H "authorization: Bearer ${BRIDGE_TOKEN_VALUE}" -H 'content-type: application/json' -d '{}'
assert_code "non-JSON body -> 400" 400 \
  -X POST "${BRIDGE_URL}/publish/dd.vapi.tasks.call" \
  -H "authorization: Bearer ${BRIDGE_TOKEN_VALUE}" -H 'content-type: application/json' -d 'nope'
python3 -c "print('{\"pad\":\"' + 'x'*8000 + '\"}')" >"${WORK_DIR}/big.json"
assert_code "oversize body -> 413" 413 \
  -X POST "${BRIDGE_URL}/publish/dd.vapi.tasks.call" \
  -H "authorization: Bearer ${BRIDGE_TOKEN_VALUE}" -H 'content-type: application/json' \
  -d "@${WORK_DIR}/big.json"
# 7, not 8: the oversize body is refused by axum's DefaultBodyLimit layer
# before the handler runs, so it never reaches the handler's reject counter.
assert_json "rejections counted, none published" \
  "d['published_total']==0 and d['rejected_total']==7" "${BRIDGE_URL}/healthz"

step "2. Durable-only subjects never downgrade to core NATS"
assert_code "durable subject without stream -> 503" 503 \
  -X POST "${BRIDGE_URL}/publish/dd.vapi.tasks.call" \
  -H "authorization: Bearer ${BRIDGE_TOKEN_VALUE}" -H 'content-type: application/json' \
  -d '{"type":"setup-refresh"}'
assert_json "durability rejection is counted" \
  "d['durability_rejected_total']==1 and d['core_published_total']==0" "${BRIDGE_URL}/healthz"

step "2b. Core-NATS fallback for an explicitly non-durable subject"
assert_json "allowed subject -> 200 durable:false" \
  "d['ok'] is True and d['durable'] is False" \
  -X POST "${BRIDGE_URL}/publish/vxl.events.test" \
  -H "authorization: Bearer ${BRIDGE_TOKEN_VALUE}" -H 'content-type: application/json' -d '{"hello":"world"}'

step "3. Vapi worker: provisioning + task lifecycle"
start_worker
wait_for 60 curl -sf "http://127.0.0.1:${VAPI_PORT}/healthz" || { echo "vapi never healthy"; exit 1; }
if wait_for 20 bash -c "curl -s '${MON_URL}/jsz?streams=true' | grep -q DD_VAPI_TASKS"; then
  ok "worker provisioned stream DD_VAPI_TASKS"
else
  bad "worker provisioned stream DD_VAPI_TASKS" "stream absent"
fi

assert_json "JetStream publish is durable-acked" "d['durable'] is True and d['duplicate'] is False" \
  -X POST "${BRIDGE_URL}/publish/dd.vapi.tasks.call" \
  -H "authorization: Bearer ${BRIDGE_TOKEN_VALUE}" -H 'content-type: application/json' \
  -d '{"type":"reboot-cluster"}'


first_dedupe="$(curl -s -X POST "${BRIDGE_URL}/publish/dd.vapi.tasks.call" \
  -H "authorization: Bearer ${BRIDGE_TOKEN_VALUE}" -H 'content-type: application/json' \
  -H 'x-message-id: e2e-vapi-dedup-001' -d '{"type":"reboot-cluster"}')"
second_dedupe="$(curl -s -X POST "${BRIDGE_URL}/publish/dd.vapi.tasks.call" \
  -H "authorization: Bearer ${BRIDGE_TOKEN_VALUE}" -H 'content-type: application/json' \
  -H 'x-message-id: e2e-vapi-dedup-001' -d '{"type":"reboot-cluster"}')"
if python3 -c 'import json,sys; first,second=map(json.loads,sys.argv[1:3]); assert first["durable"] is True and first["duplicate"] is False; assert second["durable"] is True and second["duplicate"] is True; assert first["stream"] == second["stream"] == "DD_VAPI_TASKS"; assert first["sequence"] == second["sequence"]; assert first["messageId"] == second["messageId"] == "e2e-vapi-dedup-001"' "$first_dedupe" "$second_dedupe"; then
  ok "repeated x-message-id is durably de-duplicated"
else
  bad "JetStream message-id de-duplication" "first=$first_dedupe second=$second_dedupe"
fi
if wait_for 15 grep -q "unknown task type 'reboot-cluster'" "${WORK_DIR}/vapi.log"; then
  ok "poison task dropped + acked (cannot wedge queue)"
else
  bad "poison task dropped + acked" "no drop logged"
fi

publish dd.vapi.tasks.call '{"type":"outbound-call","number":"+15551234567"}' >/dev/null
if wait_for 15 grep -q "dropping unprocessable vapi task" "${WORK_DIR}/vapi.log"; then
  ok "unprocessable task (no phone id) dropped + acked"
else
  bad "unprocessable task dropped + acked" "no drop logged"
fi

publish dd.vapi.tasks.call '{"type":"setup-refresh"}' >/dev/null
if wait_for 45 bash -c "[ \"\$(grep -c 'nak for redelivery' '${WORK_DIR}/vapi.log')\" -ge 3 ]"; then
  ok "transient failure NAK'd and redelivered to max_deliver"
else
  bad "transient failure redelivery" "count=$(grep -c 'nak for redelivery' "${WORK_DIR}/vapi.log" || true)"
fi
if wait_for 20 bash -c "[ \"\$(curl -s '${MON_URL}/jsz?consumers=true' | python3 -c \"
import json,sys
d=json.load(sys.stdin)
print(max([c.get('num_ack_pending',0) for a in d.get('account_details',[]) for s in a.get('stream_detail',[]) for c in s.get('consumer_detail',[])] or [1]))
\")\" = 0 ]"; then
  ok "redelivery is bounded (num_ack_pending back to 0)"
else
  bad "redelivery is bounded" "ack_pending=$(consumer_field num_ack_pending)"
fi

step "4. KEDA scale signal: lag accrues while worker is down, drains on return"
kill "$VAPI_PID" 2>/dev/null || true; wait "$VAPI_PID" 2>/dev/null || true; VAPI_PID=""
for i in 1 2 3 4 5; do
  publish dd.vapi.tasks.call "{\"type\":\"outbound-call\",\"number\":\"+1555123456${i}\"}" >/dev/null
done
if [ "$(consumer_field num_pending)" = "5" ]; then
  ok "consumer lag = 5 with worker down (KEDA activation threshold is 2)"
else
  bad "consumer lag accrues" "num_pending=$(consumer_field num_pending)"
fi
start_worker
if wait_for 90 bash -c "[ \"\$(curl -s '${MON_URL}/jsz?consumers=true' | python3 -c \"
import json,sys
d=json.load(sys.stdin)
print(sum(c.get('num_pending',0) for a in d.get('account_details',[]) for s in a.get('stream_detail',[]) for c in s.get('consumer_detail',[])))
\")\" = 0 ]"; then
  ok "queue drains to 0 when worker returns"
else
  bad "queue drains" "num_pending=$(consumer_field num_pending)"
fi

step "5. Resilience across a NATS restart"
docker stop "$NATS_CONTAINER" >/dev/null
if wait_for 30 bash -c "[ \"\$(curl -s -o /dev/null -w '%{http_code}' '${BRIDGE_URL}/readyz')\" = 503 ]"; then
  ok "bridge /readyz reports 503 while NATS is down"
else
  bad "bridge /readyz 503 while NATS down" "got $(curl -s -o /dev/null -w '%{http_code}' "${BRIDGE_URL}/readyz")"
fi
code="$(curl -s -o /dev/null -w '%{http_code}' -X POST "${BRIDGE_URL}/publish/dd.vapi.tasks.call" \
  -H "authorization: Bearer ${BRIDGE_TOKEN_VALUE}" -H 'content-type: application/json' -d '{"type":"setup-refresh"}' || true)"
if [ "$code" = "502" ] || [ "$code" = "504" ]; then
  ok "publish fails cleanly (HTTP $code) while NATS is down"
else
  bad "publish fails cleanly while NATS down" "got $code"
fi
docker start "$NATS_CONTAINER" >/dev/null
wait_for 30 curl -sf "${MON_URL}/healthz" || { echo "NATS did not come back"; exit 1; }
if wait_for 60 curl -sf "${BRIDGE_URL}/readyz"; then
  ok "bridge recovers /readyz after NATS returns"
else
  bad "bridge recovers after NATS returns" "still not ready"
fi
if wait_for 90 bash -c "curl -s '${BRIDGE_URL}/publish/vxl.events.recovered' \
  -X POST -H 'authorization: Bearer ${BRIDGE_TOKEN_VALUE}' -H 'content-type: application/json' \
  -d '{}' | grep -q '\"ok\":true'"; then
  ok "bridge publishes again after reconnect"
else
  bad "bridge publishes after reconnect" "publish still failing"
fi
# Recovery is asserted by BEHAVIOUR, not mechanism. Two layers can deliver it:
# async-nats reconnects the client transparently (preferred — no consumer
# re-provisioning), and failing that the worker's supervised loop rebuilds the
# consumer. Either is a pass; the log line below just records which happened.
restarts_before="$(grep -c 'vapi nats worker started' "${WORK_DIR}/vapi.log" || true)"
if wait_for 60 bash -c "publish() { curl -s -X POST '${BRIDGE_URL}/publish/dd.vapi.tasks.call' \
  -H 'authorization: Bearer ${BRIDGE_TOKEN_VALUE}' -H 'content-type: application/json' -d \"\$1\"; }; \
  publish '{\"type\":\"reboot-cluster\"}' >/dev/null; \
  [ \"\$(grep -c \"unknown task type\" '${WORK_DIR}/vapi.log')\" -ge 2 ]"; then
  restarts_after="$(grep -c 'vapi nats worker started' "${WORK_DIR}/vapi.log" || true)"
  if [ "$restarts_after" -gt "$restarts_before" ]; then
    ok "worker resumed consuming after NATS restart (via supervisor rebuild)"
  else
    ok "worker resumed consuming after NATS restart (via transparent client reconnect)"
  fi
else
  bad "worker resumes consuming after NATS restart" "no new task processed"
fi

step "6. Concurrent publish load"
before_pub="$(curl -s "${BRIDGE_URL}/healthz" | python3 -c 'import json,sys; print(json.load(sys.stdin)["published_total"])')"
seq 1 50 | xargs -P 10 -I{} curl -s -o /dev/null -X POST "${BRIDGE_URL}/publish/vxl.events.load" \
  -H "authorization: Bearer ${BRIDGE_TOKEN_VALUE}" -H 'content-type: application/json' -d '{"n":{}}'
after_pub="$(curl -s "${BRIDGE_URL}/healthz" | python3 -c 'import json,sys; print(json.load(sys.stdin)["published_total"])')"
if [ "$(( after_pub - before_pub ))" = "50" ]; then
  ok "50 concurrent publishes all counted exactly once"
else
  bad "concurrent publish counter integrity" "delta=$(( after_pub - before_pub ))"
fi
if kill -0 "$BRIDGE_PID" 2>/dev/null; then ok "bridge still alive after load"; else bad "bridge alive after load" "process died"; fi

step "Results"
printf '  %d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || { echo; echo "--- bridge.log tail ---"; tail -20 "${WORK_DIR}/bridge.log"; \
  echo "--- vapi.log tail ---"; tail -20 "${WORK_DIR}/vapi.log"; exit 1; }
