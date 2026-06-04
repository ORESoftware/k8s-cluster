#!/usr/bin/env bash
# Build the first-party AI/ML platform images into the EC2 Kubernetes
# containerd namespace. Run from any directory in this checkout.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
CONTAINER_NAMESPACE="${CONTAINER_NAMESPACE:-k8s.io}"
BUILDER="${BUILDER:-nerdctl}"
PROGRESS="${PROGRESS:-plain}"

if ! command -v "${BUILDER}" >/dev/null 2>&1; then
  echo "required container builder is unavailable: ${BUILDER}" >&2
  exit 127
fi

build_image() {
  local dockerfile="$1"
  local tag="$2"

  echo "building ${tag} from ${dockerfile}"
  "${BUILDER}" -n "${CONTAINER_NAMESPACE}" build \
    --progress="${PROGRESS}" \
    -f "${REPO_ROOT}/${dockerfile}" \
    -t "${tag}" \
    "${REPO_ROOT}"
}

build_image \
  "remote/deployments/ai-ml-pipeline/Dockerfile" \
  "docker.io/library/dd-ai-ml-pipeline:dev"

build_image \
  "remote/deployments/spark-pipeline-server/Dockerfile" \
  "docker.io/library/dd-spark-pipeline-server:dev"

echo "AI/ML platform images are present in containerd namespace ${CONTAINER_NAMESPACE}"
