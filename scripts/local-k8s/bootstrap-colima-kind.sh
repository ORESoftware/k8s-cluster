#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PROFILE_PATH="${LOCAL_K8S_PROFILE:-}"

if [[ -z "${PROFILE_PATH}" ]]; then
  printf '%s\n' 'LOCAL_K8S_PROFILE is required' >&2
  exit 2
fi

for command_name in python3 colima docker kind kubectl; do
  command -v "${command_name}" >/dev/null 2>&1 || {
    printf 'required command is unavailable: %s\n' "${command_name}" >&2
    exit 2
  }
done

LOCAL_K8S_PROFILE="${PROFILE_PATH}" python3 "${ROOT_DIR}/scripts/local-k8s/verify_profile.py" >/dev/null

profile_value() {
  LOCAL_K8S_PROFILE_PATH="${PROFILE_PATH}" LOCAL_K8S_PROFILE_KEY="$1" python3 - <<'PY'
import json
import os
from pathlib import Path

profile = json.loads(Path(os.environ["LOCAL_K8S_PROFILE_PATH"]).read_text(encoding="utf-8"))
value = profile[os.environ["LOCAL_K8S_PROFILE_KEY"]]
if not isinstance(value, str):
    raise SystemExit("requested profile value is not a string")
print(value)
PY
}

NAME="$(profile_value name)"
RUNTIME="$(profile_value runtime)"
ARCHITECTURE="$(profile_value architecture)"
RESOURCE_CLASS="$(profile_value resourceClass)"
TOPOLOGY="$(profile_value topology)"
NODE_IMAGE="$(profile_value runtimeArtifact)"

if [[ "${RUNTIME}" != 'colima-kind' ]]; then
  printf 'profile runtime must be colima-kind; got %s\n' "${RUNTIME}" >&2
  exit 2
fi

case "${RESOURCE_CLASS}" in
  compact) CPU_COUNT=2; MEMORY_GIB=4; DISK_GIB=40 ;;
  standard) CPU_COUNT=4; MEMORY_GIB=6; DISK_GIB=60 ;;
  large) CPU_COUNT=6; MEMORY_GIB=10; DISK_GIB=100 ;;
  *) printf 'unreachable resource class: %s\n' "${RESOURCE_CLASS}" >&2; exit 2 ;;
esac

case "${TOPOLOGY}" in
  single-node) WORKER_COUNT=0 ;;
  three-node) WORKER_COUNT=2 ;;
  *) printf 'unreachable topology: %s\n' "${TOPOLOGY}" >&2; exit 2 ;;
esac

case "${ARCHITECTURE}" in
  arm64) COLIMA_ARCH='aarch64' ;;
  amd64) COLIMA_ARCH='x86_64' ;;
  *) printf 'unreachable architecture: %s\n' "${ARCHITECTURE}" >&2; exit 2 ;;
esac

COLIMA_PROFILE="ores-${NAME}"
DOCKER_CONTEXT_NAME="colima-${COLIMA_PROFILE}"
CLUSTER_NAME="${NAME}"
RUNTIME_DIR="${ROOT_DIR}/tmp/local-k8s/${NAME}"
KUBECONFIG_PATH="${RUNTIME_DIR}/kubeconfig"
KIND_CONFIG_PATH="${RUNTIME_DIR}/kind-config.yaml"
mkdir -p "${RUNTIME_DIR}"

colima start "${COLIMA_PROFILE}" \
  --runtime docker \
  --cpus "${CPU_COUNT}" \
  --memory "${MEMORY_GIB}" \
  --disk "${DISK_GIB}" \
  --arch "${COLIMA_ARCH}" \
  --activate=false

export DOCKER_CONTEXT="${DOCKER_CONTEXT_NAME}"
docker version > "${RUNTIME_DIR}/docker-version.txt"
docker info > "${RUNTIME_DIR}/docker-info.txt"

if kind get clusters 2>/dev/null | grep -Fxq "${CLUSTER_NAME}"; then
  printf 'refusing to reuse existing kind cluster %s in Docker context %s\n' \
    "${CLUSTER_NAME}" "${DOCKER_CONTEXT_NAME}" >&2
  exit 2
fi

{
  printf '%s\n' 'kind: Cluster'
  printf '%s\n' 'apiVersion: kind.x-k8s.io/v1alpha4'
  printf '%s\n' 'nodes:'
  printf '%s\n' '  - role: control-plane'
  for ((index = 0; index < WORKER_COUNT; index += 1)); do
    printf '%s\n' '  - role: worker'
  done
} > "${KIND_CONFIG_PATH}"

kind create cluster \
  --name "${CLUSTER_NAME}" \
  --image "${NODE_IMAGE}" \
  --config "${KIND_CONFIG_PATH}" \
  --kubeconfig "${KUBECONFIG_PATH}" \
  --wait 180s

kubectl --kubeconfig "${KUBECONFIG_PATH}" wait \
  --for=condition=Ready nodes --all --timeout=180s
kubectl --kubeconfig "${KUBECONFIG_PATH}" get nodes -o wide \
  > "${RUNTIME_DIR}/nodes.txt"
kubectl --kubeconfig "${KUBECONFIG_PATH}" get --raw='/readyz?verbose' \
  > "${RUNTIME_DIR}/apiserver-readyz.txt"

printf 'local kind cluster ready: %s\n' "${CLUSTER_NAME}"
printf 'docker context: %s\n' "${DOCKER_CONTEXT_NAME}"
printf 'kubeconfig: %s\n' "${KUBECONFIG_PATH}"
printf '%s\n' 'cloud mutations: denied'
