#!/usr/bin/env bash
set -euo pipefail

# Smoke-test the dd-runtime-config rollout from inside the live Kubernetes
# cluster. Default mode is read-only. Set DD_RUNTIME_CONFIG_MUTATE=1 to run a
# real upsert -> push -> subscriber snapshot -> delete round trip.

KX="${KX:-kubectl}"
ENVIRONMENT="${DD_RUNTIME_CONFIG_ENV:-stage}"
NAMESPACE="${DD_RUNTIME_CONFIG_NAMESPACE:-default}"
MUTATE="${DD_RUNTIME_CONFIG_MUTATE:-0}"

runtime_url="http://dd-runtime-config.${NAMESPACE}.svc.cluster.local:8110"

default_subscribers=(
  "default|dd-agent-worker-broker|http://dd-agent-worker-broker.default.svc.cluster.local:8098"
  "ai-ml|dd-ai-ml-pipeline|http://dd-ai-ml-pipeline.ai-ml.svc.cluster.local:8099"
  "vpn|dd-bastion|http://dd-bastion.vpn.svc.cluster.local:8111"
  "default|dd-build-server|http://dd-build-server.default.svc.cluster.local:8100"
  "default|dd-container-pool|http://dd-container-pool.default.svc.cluster.local:8102"
  "default|dd-contract-service|http://dd-contract-service.default.svc.cluster.local:8101"
  "default|dd-des-simulator|http://dd-des-simulator.default.svc.cluster.local:8099"
  "default|dd-dev-server-api|http://dd-dev-server-api.default.svc.cluster.local:8080"
  "default|dd-formal-methods-server|http://dd-formal-methods-server.default.svc.cluster.local:8110"
  "default|dd-formal-methods-service|http://dd-formal-methods-service.default.svc.cluster.local:8111"
  "default|dd-gleam-lambda-runner|http://dd-gleam-lambda-runner.default.svc.cluster.local:8083"
  "default|dd-gleam-mcp-server|http://dd-gleam-mcp-server.default.svc.cluster.local:8090"
  "default|dd-gleamlang-server|http://dd-gleamlang-server.default.svc.cluster.local:8081"
  "default|dd-gleamlang-ws-server|http://dd-gleamlang-ws-server.default.svc.cluster.local:8081"
  "default|dd-mdp-optimizer|http://dd-mdp-optimizer.default.svc.cluster.local:8096"
  "default|dd-remote-auth|http://dd-remote-auth.default.svc.cluster.local:8083"
  "default|dd-remote-rest-api|http://dd-remote-rest-api.default.svc.cluster.local:8082"
  "default|dd-remote-web-home|http://dd-remote-web-home.default.svc.cluster.local:8080"
  "default|dd-trading-server|http://dd-trading-server.default.svc.cluster.local:8103"
  "default|dd-webrtc-media|http://dd-webrtc-media.default.svc.cluster.local:8125"
  "default|dd-webrtc-signaling|http://dd-webrtc-signaling.default.svc.cluster.local:8095"
)

require() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "missing required command: $1" >&2
    exit 1
  fi
}

json_has_subscriber() {
  local json="$1"
  local name="$2"
  node -e '
    const fs = require("node:fs");
    const payload = JSON.parse(fs.readFileSync(0, "utf8"));
    const name = process.argv[1];
    if (!payload.subscribers?.some((sub) => sub.name === name)) process.exit(1);
  ' "$name" <<<"$json"
}

require "$KX"
require node

echo "== rollout"
"$KX" -n "$NAMESPACE" rollout status deploy/dd-runtime-config --timeout=180s
"$KX" -n "$NAMESPACE" get deploy/dd-runtime-config

runtime_pod="$("$KX" -n "$NAMESPACE" get pod -l app=dd-runtime-config -o jsonpath='{.items[0].metadata.name}')"
if [[ -z "$runtime_pod" ]]; then
  echo "dd-runtime-config pod not found" >&2
  exit 1
fi

echo "== health"
"$KX" -n "$NAMESPACE" exec "$runtime_pod" -- curl -sf "$runtime_url/healthz" >/dev/null
"$KX" -n "$NAMESPACE" exec "$runtime_pod" -- curl -sf "$runtime_url/metrics" | head -20

echo "== subscriber registry"
subs_json="$("$KX" -n "$NAMESPACE" exec "$runtime_pod" -- curl -sf "$runtime_url/subscribers/$ENVIRONMENT")"
for item in "${default_subscribers[@]}"; do
  IFS='|' read -r ns name url <<<"$item"
  if json_has_subscriber "$subs_json" "$name"; then
    echo "registered: $name"
  else
    echo "missing subscriber registration: $name" >&2
    exit 1
  fi
done

echo "== apply route reachability"
for item in "${default_subscribers[@]}"; do
  IFS='|' read -r ns name url <<<"$item"
  "$KX" -n "$NAMESPACE" exec "$runtime_pod" -- \
    curl -sf "$url/internal/runtime-config" >/dev/null
  echo "reachable: $name"
done

echo "== presence replicas"
presence_pods="$("$KX" -n presence get pod -l app=dd-gleamlang-presence-server -o name 2>/dev/null || true)"
if [[ -n "$presence_pods" ]]; then
  while IFS= read -r pod_ref; do
    pod="${pod_ref#pod/}"
    url="http://${pod}.presence-svc.presence.svc.cluster.local:8081/internal/runtime-config"
    "$KX" -n "$NAMESPACE" exec "$runtime_pod" -- curl -sf "$url" >/dev/null
    echo "reachable: $pod"
  done <<<"$presence_pods"
else
  echo "presence namespace has no matching pods; skipping replica-specific reachability"
fi

if [[ "$MUTATE" != "1" ]]; then
  echo "read-only smoke ok. Set DD_RUNTIME_CONFIG_MUTATE=1 for upsert/push/delete round trip."
  exit 0
fi

echo "== mutation round trip"
secret="$("$KX" -n "$NAMESPACE" get secret dd-agent-secrets -o jsonpath='{.data.SERVER_AUTH_SECRET}' | base64 -d)"
key="SMOKE_RUNTIME_CONFIG_$(date +%s)"
body="$(node -e '
  const key = process.argv[1];
  process.stdout.write(JSON.stringify({
    env: process.env.DD_RUNTIME_CONFIG_ENV || "stage",
    scope: "dd-remote-web-home",
    key,
    value: { ok: true, source: "runtime-config-cluster-smoke" },
    reason: "cluster smoke test"
  }));
' "$key")"

"$KX" -n "$NAMESPACE" exec "$runtime_pod" -- \
  curl -sf -X POST "$runtime_url/entries/$ENVIRONMENT" \
    -H "X-Server-Auth: $secret" \
    -H 'content-type: application/json' \
    -d "$body" >/dev/null

"$KX" -n "$NAMESPACE" exec "$runtime_pod" -- \
  curl -sf -X POST "$runtime_url/push/$ENVIRONMENT" \
    -H "X-Server-Auth: $secret" >/tmp/dd-runtime-config-push.json

node -e '
  const fs = require("node:fs");
  const payload = JSON.parse(fs.readFileSync("/tmp/dd-runtime-config-push.json", "utf8"));
  const web = payload.outcomes?.find((outcome) => outcome.subscriber === "dd-remote-web-home");
  if (!web?.ok) {
    console.error(JSON.stringify(payload, null, 2));
    process.exit(1);
  }
'

"$KX" -n "$NAMESPACE" exec "$runtime_pod" -- \
  curl -sf "http://dd-remote-web-home.default.svc.cluster.local:8080/internal/runtime-config" \
  | node -e '
      const fs = require("node:fs");
      const key = process.argv[1];
      const payload = JSON.parse(fs.readFileSync(0, "utf8"));
      if (!payload.entries || !Object.prototype.hasOwnProperty.call(payload.entries, key)) {
        console.error(JSON.stringify(payload, null, 2));
        process.exit(1);
      }
    ' "$key"

"$KX" -n "$NAMESPACE" exec "$runtime_pod" -- \
  curl -sf -X DELETE "$runtime_url/entries/$ENVIRONMENT/dd-remote-web-home/$key" \
    -H "X-Server-Auth: $secret" >/dev/null

echo "mutation smoke ok: $key"
