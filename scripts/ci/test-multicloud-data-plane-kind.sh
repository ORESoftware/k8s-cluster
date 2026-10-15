#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CLUSTER_NAME="${MULTICLOUD_KIND_CLUSTER_NAME:-multicloud-data-plane-contract}"
CLUSTER_CREATED=0

cleanup() {
  local status=$?
  trap - EXIT
  if [[ "${CLUSTER_CREATED}" == "1" ]]; then
    kind delete cluster --name "${CLUSTER_NAME}" >/dev/null 2>&1 || true
  fi
  exit "${status}"
}
trap cleanup EXIT

for command in awk helm kind kubectl mktemp; do
  command -v "${command}" >/dev/null 2>&1 || {
    echo "required command is unavailable: ${command}" >&2
    exit 1
  }
done

cd "${ROOT_DIR}"
kind create cluster --name "${CLUSTER_NAME}" --wait 120s
CLUSTER_CREATED=1

for namespace in cert-manager external-secrets cockroachdb cockroach-operator nats-system; do
  kubectl create namespace "${namespace}" >/dev/null
done

# Install only the exact CRD schemas used by the pinned charts. Controllers and
# admission webhooks are intentionally omitted: this test exercises native API
# discovery, structural schemas, and strict server-side field validation without
# requiring cloud credentials or reconciling any stateful workloads.
helm template cert-manager cert-manager \
  --repo https://charts.jetstack.io \
  --version v1.21.1 \
  --namespace cert-manager \
  --include-crds \
  --set crds.enabled=true \
  --set crds.keep=true \
  | awk 'BEGIN { RS="---"; ORS="---\n" } /kind: CustomResourceDefinition/ { print }' \
  | kubectl apply -f - >/dev/null

helm template trust-manager trust-manager \
  --repo https://charts.jetstack.io \
  --version v0.24.0 \
  --namespace cert-manager \
  --include-crds \
  --set crds.enabled=true \
  --set crds.keep=true \
  | awk 'BEGIN { RS="---"; ORS="---\n" } /kind: CustomResourceDefinition/ { print }' \
  | kubectl apply -f - >/dev/null

helm template external-secrets external-secrets \
  --repo https://charts.external-secrets.io \
  --version 0.18.2 \
  --namespace external-secrets \
  --include-crds \
  --set installCRDs=true \
  | awk 'BEGIN { RS="---"; ORS="---\n" } /kind: CustomResourceDefinition/ { print }' \
  | kubectl apply -f - >/dev/null

operator_chart_dir="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/crdb-operator-chart.XXXXXX")"
helm pull cockroachdb-operator-chart \
  --repo https://charts.cockroachdb.com/v2 \
  --version 1.0.0 \
  --untar \
  --untardir "${operator_chart_dir}"
kubectl apply \
  -f "${operator_chart_dir}/cockroachdb-operator-chart/manifests/crds" >/dev/null

kubectl wait --for=condition=Established crd --all --timeout=120s >/dev/null

kubectl kustomize remote/argocd/multicloud-data-plane/prerequisites \
  | kubectl apply --dry-run=server --validate=strict -f - >/dev/null

helm template dd-cockroachdb-operator cockroachdb-operator-chart \
  --repo https://charts.cockroachdb.com/v2 \
  --version 1.0.0 \
  --namespace cockroach-operator \
  --set cloudRegion=us-east-1 \
  --set watchNamespaces=cockroachdb \
  | kubectl apply --dry-run=server --validate=strict -f - >/dev/null

for provider in aws gcp azure; do
  helm template dd-cockroachdb cockroachdb-chart \
    --repo https://charts.cockroachdb.com/v2 \
    --version 26.3.1 \
    --namespace cockroachdb \
    -f remote/argocd/multicloud-data-plane/cockroachdb/values/common.yaml \
    -f "remote/argocd/multicloud-data-plane/cockroachdb/values/${provider}.yaml" \
    | kubectl apply --dry-run=server --validate=strict -f - >/dev/null

  helm template dd-nats-supercluster nats \
    --repo https://nats-io.github.io/k8s/helm/charts/ \
    --version 2.14.6 \
    --namespace nats-system \
    -f remote/argocd/multicloud-data-plane/nats/values/common.yaml \
    -f "remote/argocd/multicloud-data-plane/nats/values/${provider}.yaml" \
    | kubectl apply --dry-run=server --validate=strict -f - >/dev/null
done

echo "PASS: pinned CRDs accepted prerequisites plus all AWS/GCP/Azure CockroachDB and NATS payloads."
