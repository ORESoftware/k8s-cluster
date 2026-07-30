#!/usr/bin/env bash
# Build the native-library Rust services into the Kubernetes containerd namespace.
# Image names are derived from each crate's [package] version and must stay in
# exact sync with every checked-in GitOps and warm-pool runtime consumer.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
REMOTE_ROOT="${REPO_ROOT}/remote"
CONTAINER_NAMESPACE="${CONTAINER_NAMESPACE:-k8s.io}"
BUILDER="${BUILDER:-nerdctl}"
PROGRESS="${PROGRESS:-plain}"
IMAGE_PREFIX="${IMAGE_PREFIX:-docker.io/library}"
ALLOW_EXISTING_IMAGE="${ALLOW_EXISTING_IMAGE:-0}"
ALLOW_DIRTY_BUILD="${ALLOW_DIRTY_BUILD:-0}"

readonly -a SERVICES=(dd-document-rs dd-ocr-rs)

usage() {
  cat >&2 <<'EOF'
usage: remote/tools/build-native-service-images.sh [check|build]

  check  Verify Cargo versions, immutable GitOps/warm-pool image references,
         pull policy, and native-service Dockerfile release contracts. Default.
  build  Run the checks, then build each exact version tag into containerd's
         Kubernetes namespace and verify that the resulting image exists.

Environment:
  BUILDER=nerdctl                 Container builder binary.
  CONTAINER_NAMESPACE=k8s.io      nerdctl/containerd namespace.
  IMAGE_PREFIX=docker.io/library  Image name prefix used by runtime consumers.
  PROGRESS=plain                  BuildKit progress mode.
  ALLOW_EXISTING_IMAGE=1          Explicit emergency override for a tag rebuild.
  ALLOW_DIRTY_BUILD=1             Explicit local-development override.
EOF
  exit 64
}

package_version() {
  local cargo_toml="$1"
  awk '
    /^\[package\][[:space:]]*$/ { in_package = 1; next }
    in_package && /^\[/ { exit }
    in_package && /^[[:space:]]*version[[:space:]]*=/ {
      line = $0
      sub(/^[^=]*=[[:space:]]*"/, "", line)
      sub(/".*/, "", line)
      print line
      exit
    }
  ' "${cargo_toml}"
}

service_version() {
  local service="$1"
  local version
  version="$(package_version "${REMOTE_ROOT}/deployments/${service}/Cargo.toml")"
  if [[ ! "${version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+([+-][0-9A-Za-z.-]+)?$ ]]; then
    echo "invalid or missing [package] version for ${service}: ${version:-<empty>}" >&2
    return 1
  fi
  printf '%s\n' "${version}"
}

service_image() {
  local service="$1"
  printf '%s/%s:%s\n' "${IMAGE_PREFIX}" "${service}" "$(service_version "${service}")"
}

service_manifest() {
  local service="$1"
  printf '%s/argocd/dd-next-runtime/%s.deployment.yaml\n' "${REMOTE_ROOT}" "${service}"
}

require_exact_line() {
  local file="$1"
  local expected="$2"
  if ! grep -Fqx "${expected}" "${file}"; then
    echo "${file#${REPO_ROOT}/} must contain the exact line: ${expected}" >&2
    return 1
  fi
}

check_service_contract() {
  local service="$1"
  local image manifest dockerfile
  image="$(service_image "${service}")"
  manifest="$(service_manifest "${service}")"
  dockerfile="${REMOTE_ROOT}/deployments/${service}/Dockerfile"

  require_exact_line "${manifest}" "          image: ${image}"
  require_exact_line "${manifest}" "          imagePullPolicy: Never"

  if grep -En "image:[[:space:]]+([^[:space:]]+/)?${service}:(latest|dev|main|master)([[:space:]]|$)" "${manifest}"; then
    echo "${manifest#${REPO_ROOT}/} contains a mutable ${service} image tag" >&2
    return 1
  fi

  require_exact_line "${dockerfile}" '# syntax=docker/dockerfile:1'
  if ! grep -Fq 'cargo build --release --locked' "${dockerfile}"; then
    echo "${dockerfile#${REPO_ROOT}/} must use cargo build --release --locked" >&2
    return 1
  fi

  printf 'verified %s -> %s\n' "${service}" "${image}"
}

check_warm_pool_contract() {
  local seed document_image
  seed="${REMOTE_ROOT}/databases/pg/seeds/container-pool-app-config.sql"
  document_image="$(service_image dd-document-rs)"

  require_exact_line "${seed}" "        \"image\": \"${document_image}\","
  if grep -En '"image":[[:space:]]+"([^"[:space:]]+/)?dd-document-rs:(latest|dev|main|master)"' "${seed}"; then
    echo "${seed#${REPO_ROOT}/} contains a mutable dd-document-rs warm-pool image tag" >&2
    return 1
  fi

  printf 'verified dd-document warm pool -> %s\n' "${document_image}"
}

check_contract() {
  local service
  for service in "${SERVICES[@]}"; do
    check_service_contract "${service}"
  done
  check_warm_pool_contract
}

require_clean_inputs() {
  if [[ "${ALLOW_DIRTY_BUILD}" == "1" ]]; then
    return 0
  fi

  local status
  status="$(git -C "${REPO_ROOT}" status --porcelain --untracked-files=normal -- \
    remote/deployments/dd-document-rs \
    remote/deployments/dd-ocr-rs \
    remote/argocd/dd-next-runtime/dd-document-rs.deployment.yaml \
    remote/argocd/dd-next-runtime/dd-ocr-rs.deployment.yaml \
    remote/databases/pg/seeds/container-pool-app-config.sql \
    remote/tools/build-native-service-images.sh)"
  if [[ -n "${status}" ]]; then
    echo 'refusing to build native release tags from dirty release inputs:' >&2
    printf '%s\n' "${status}" >&2
    echo 'commit the inputs first, or set ALLOW_DIRTY_BUILD=1 for an explicit non-release build' >&2
    return 1
  fi
}

build_service() {
  local service="$1"
  local image dockerfile revision
  image="$(service_image "${service}")"
  dockerfile="${REMOTE_ROOT}/deployments/${service}/Dockerfile"
  revision="$(git -C "${REPO_ROOT}" rev-parse --verify HEAD)"

  if [[ "${ALLOW_EXISTING_IMAGE}" != "1" ]] && \
     "${BUILDER}" -n "${CONTAINER_NAMESPACE}" image inspect "${image}" >/dev/null 2>&1; then
    echo "refusing to overwrite existing release image ${image}; bump the crate version" >&2
    return 1
  fi

  echo "building ${image} from ${dockerfile#${REPO_ROOT}/}"
  "${BUILDER}" -n "${CONTAINER_NAMESPACE}" build \
    --progress="${PROGRESS}" \
    --label "org.opencontainers.image.source=https://github.com/ORESoftware/k8s-cluster" \
    --label "org.opencontainers.image.revision=${revision}" \
    --label "org.opencontainers.image.version=$(service_version "${service}")" \
    -f "${dockerfile}" \
    -t "${image}" \
    "${REMOTE_ROOT}"

  "${BUILDER}" -n "${CONTAINER_NAMESPACE}" image inspect "${image}" >/dev/null
  echo "verified ${image} in containerd namespace ${CONTAINER_NAMESPACE}"
}

mode="${1:-check}"
case "${mode}" in
  check)
    check_contract
    ;;
  build)
    check_contract
    require_clean_inputs
    if ! command -v "${BUILDER}" >/dev/null 2>&1; then
      echo "required container builder is unavailable: ${BUILDER}" >&2
      exit 127
    fi
    for service in "${SERVICES[@]}"; do
      build_service "${service}"
    done
    ;;
  *)
    usage
    ;;
esac
