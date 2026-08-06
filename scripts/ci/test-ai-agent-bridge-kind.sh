#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CLUSTER_NAME="${AI_BRIDGE_KIND_CLUSTER_NAME:-ai-agent-bridge-smoke}"
EVIDENCE_DIR="${AI_BRIDGE_EVIDENCE_DIR:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}/ai-agent-bridge-kind}"
FIXTURE="${ROOT_DIR}/remote/tests/fixtures/ai-agent-bridge-kind.yaml.tmpl"
RUNTIME_SMOKE="${ROOT_DIR}/remote/tests/general/ai-agent-bridge-runtime-smoke.mjs"
HTTP_FORWARD_PORT="${AI_BRIDGE_HTTP_FORWARD_PORT:-18142}"
TCP_FORWARD_PORT="${AI_BRIDGE_TCP_FORWARD_PORT:-18143}"
PORT_FORWARD_PID=""
CLUSTER_CREATED=0

mkdir -p "${EVIDENCE_DIR}"

require_command() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "required command is unavailable: $1" >&2
    return 1
  }
}

capture_diagnostics() {
  if [[ "${CLUSTER_CREATED}" != "1" ]]; then
    return 0
  fi
  kubectl get deployment,replicaset,pod,service -o wide \
    > "${EVIDENCE_DIR}/failure-resources.txt" 2>&1 || true
  kubectl describe deployment/dd-ai-agent-bridge \
    > "${EVIDENCE_DIR}/deployment-describe.txt" 2>&1 || true
  kubectl describe pod -l app=dd-ai-agent-bridge \
    > "${EVIDENCE_DIR}/pod-describe.txt" 2>&1 || true
  kubectl logs -l app=dd-ai-agent-bridge --all-containers \
    > "${EVIDENCE_DIR}/failure-container.log" 2>&1 || true
  kubectl get events --sort-by=.lastTimestamp \
    > "${EVIDENCE_DIR}/events.txt" 2>&1 || true
}

cleanup() {
  local status=$?
  trap - EXIT
  if [[ "${status}" -ne 0 ]]; then
    capture_diagnostics
  fi
  if [[ -n "${PORT_FORWARD_PID}" ]]; then
    kill "${PORT_FORWARD_PID}" >/dev/null 2>&1 || true
    wait "${PORT_FORWARD_PID}" >/dev/null 2>&1 || true
  fi
  if [[ "${CLUSTER_CREATED}" == "1" ]]; then
    kind delete cluster --name "${CLUSTER_NAME}" >/dev/null 2>&1 || true
  fi
  exit "${status}"
}
trap cleanup EXIT

: "${GHCR_TOKEN:?GHCR_TOKEN is required}"
: "${AI_BRIDGE_TOKEN:?AI_BRIDGE_TOKEN is required}"

for command in docker kubectl kind node curl git sed grep; do
  require_command "${command}"
done
[[ -f "${FIXTURE}" ]] || {
  echo "Kubernetes fixture is missing: ${FIXTURE}" >&2
  exit 1
}
[[ -f "${RUNTIME_SMOKE}" ]] || {
  echo "runtime smoke is missing: ${RUNTIME_SMOKE}" >&2
  exit 1
}

cd "${ROOT_DIR}"
source_sha="$(git ls-tree HEAD remote/deployments/ai-agent-bridge | awk '{print $3}')"
[[ "${source_sha}" =~ ^[0-9a-f]{40}$ ]] || {
  echo "invalid ai-agent-bridge gitlink SHA: ${source_sha}" >&2
  exit 1
}
image_tag="ghcr.io/oresoftware/fiducia-ai-agent-bridge:sha-${source_sha}"

docker_config="$(mktemp -d "${RUNNER_TEMP:-/tmp}/ai-bridge-docker.XXXXXX")"
export DOCKER_CONFIG="${docker_config}"
printf '%s' "${GHCR_TOKEN}" | docker login ghcr.io -u "${GITHUB_ACTOR:-ci}" --password-stdin >/dev/null
docker pull "${image_tag}" >/dev/null

image_digest="$(docker image inspect --format '{{index .RepoDigests 0}}' "${image_tag}")"
image_revision="$(docker image inspect --format '{{index .Config.Labels "org.opencontainers.image.revision"}}' "${image_tag}")"
image_user="$(docker image inspect --format '{{.Config.User}}' "${image_tag}")"
image_entrypoint="$(docker image inspect --format '{{index .Config.Entrypoint 0}}' "${image_tag}")"
[[ "${image_revision}" == "${source_sha}" ]]
[[ "${image_user}" == "nonroot:nonroot" ]]
[[ "${image_entrypoint}" == "/usr/local/bin/fiducia-ai-agent-bridge" ]]
rm -rf "${docker_config}"
unset DOCKER_CONFIG

cat > "${EVIDENCE_DIR}/image-contract.txt" <<EOF
source_sha=${source_sha}
image_digest=${image_digest}
runtime_user=${image_user}
kubernetes_run_as_user=65532
kubernetes_run_as_group=65532
kubernetes_fs_group=65532
entrypoint=${image_entrypoint}
EOF

kind create cluster --name "${CLUSTER_NAME}" --wait 120s
CLUSTER_CREATED=1
kind load docker-image --name "${CLUSTER_NAME}" "${image_tag}"
kubectl cluster-info > "${EVIDENCE_DIR}/cluster-info.txt"
kubectl get nodes -o wide > "${EVIDENCE_DIR}/nodes.txt"

kubectl create secret generic dd-ai-agent-bridge-secrets \
  --from-literal=inbox_token="${AI_BRIDGE_TOKEN}" \
  --dry-run=client -o yaml | kubectl apply -f - >/dev/null

manifest="${EVIDENCE_DIR}/deployment-service.yaml"
sed "s|__AI_BRIDGE_IMAGE__|${image_tag}|g" "${FIXTURE}" > "${manifest}"
if grep -Fq '__AI_BRIDGE_IMAGE__' "${manifest}"; then
  echo "image placeholder was not replaced" >&2
  exit 1
fi
kubectl apply --server-side --field-manager=ai-agent-bridge-kind-smoke -f "${manifest}" >/dev/null
kubectl rollout status deployment/dd-ai-agent-bridge --timeout=120s
kubectl wait --for=condition=Ready pod -l app=dd-ai-agent-bridge --timeout=60s

kubectl get deployment,replicaset,pod,service -l app=dd-ai-agent-bridge -o wide \
  > "${EVIDENCE_DIR}/resources.txt"
kubectl get pod -l app=dd-ai-agent-bridge -o json \
  > "${EVIDENCE_DIR}/pods.json"

kubectl port-forward service/dd-ai-agent-bridge \
  "${HTTP_FORWARD_PORT}:8142" "${TCP_FORWARD_PORT}:8143" \
  > "${EVIDENCE_DIR}/port-forward.log" 2>&1 &
PORT_FORWARD_PID=$!

for _ in $(seq 1 30); do
  if curl --fail --silent --show-error \
    "http://127.0.0.1:${HTTP_FORWARD_PORT}/healthz" \
    > "${EVIDENCE_DIR}/healthz.json"; then
    break
  fi
  if ! kill -0 "${PORT_FORWARD_PID}" >/dev/null 2>&1; then
    echo "kubectl port-forward exited before health became available" >&2
    exit 1
  fi
  sleep 1
done
curl --fail --silent --show-error \
  "http://127.0.0.1:${HTTP_FORWARD_PORT}/readyz" \
  > "${EVIDENCE_DIR}/readyz.json"

AI_BRIDGE_HTTP_URL="http://127.0.0.1:${HTTP_FORWARD_PORT}/" \
AI_BRIDGE_TCP_HOST=127.0.0.1 \
AI_BRIDGE_TCP_PORT="${TCP_FORWARD_PORT}" \
AI_BRIDGE_TOKEN="${AI_BRIDGE_TOKEN}" \
node "${RUNTIME_SMOKE}" 2>&1 | tee "${EVIDENCE_DIR}/runtime-smoke.log"

pod="$(kubectl get pod -l app=dd-ai-agent-bridge -o jsonpath='{.items[0].metadata.name}')"
[[ -n "${pod}" ]]
[[ "$(kubectl get pod "${pod}" -o jsonpath='{.status.containerStatuses[0].ready}')" == "true" ]]
[[ "$(kubectl get pod "${pod}" -o jsonpath='{.spec.automountServiceAccountToken}')" == "false" ]]
[[ "$(kubectl get pod "${pod}" -o jsonpath='{.spec.securityContext.runAsUser}')" == "65532" ]]
[[ "$(kubectl get pod "${pod}" -o jsonpath='{.spec.securityContext.runAsGroup}')" == "65532" ]]
[[ "$(kubectl get pod "${pod}" -o jsonpath='{.spec.securityContext.fsGroup}')" == "65532" ]]
[[ "$(kubectl get pod "${pod}" -o jsonpath='{.spec.containers[0].securityContext.allowPrivilegeEscalation}')" == "false" ]]
[[ "$(kubectl get pod "${pod}" -o jsonpath='{.spec.containers[0].securityContext.readOnlyRootFilesystem}')" == "true" ]]
[[ "$(kubectl get pod "${pod}" -o jsonpath='{.spec.containers[0].securityContext.runAsUser}')" == "65532" ]]
[[ "$(kubectl get pod "${pod}" -o jsonpath='{.spec.containers[0].image}')" == "${image_tag}" ]]
kubectl logs "${pod}" > "${EVIDENCE_DIR}/container.log"

for file in "${EVIDENCE_DIR}"/*; do
  [[ -f "${file}" ]] || continue
  if grep -Fq "${AI_BRIDGE_TOKEN}" "${file}"; then
    echo "credential material found in evidence: ${file}" >&2
    exit 1
  fi
done

cat > "${EVIDENCE_DIR}/summary.json" <<EOF
{
  "ok": true,
  "source_sha": "${source_sha}",
  "image_digest": "${image_digest}",
  "deployment_ready": true,
  "probes": ["startup", "readiness", "liveness"],
  "service_ports": [8142, 8143],
  "transports": ["http", "sse", "tcp"],
  "workflow_modes": ["single", "sequential", "competitive", "consensus"],
  "local_lease_cycle": true,
  "legacy_inbox_auth": true,
  "run_as_user": 65532,
  "run_as_group": 65532,
  "fs_group": 65532,
  "credential_leak_scan": true
}
EOF

if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
  {
    echo '## AI bridge ephemeral Kubernetes smoke'
    echo
    echo "- source revision: \`${source_sha}\`"
    echo "- image: \`${image_digest}\`"
    echo '- Deployment rollout and pod readiness: passed'
    echo '- startup, readiness, and liveness probes: passed'
    echo '- ClusterIP HTTP and TCP service: passed'
    echo '- HTTP, SSE, TCP, workflows, lease cycle, and legacy auth: passed'
    echo '- UID/GID/fsGroup 65532 and hardened container security: passed'
    echo '- synthetic credential leakage scan: passed'
  } >> "${GITHUB_STEP_SUMMARY}"
fi
