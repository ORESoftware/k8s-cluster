#!/usr/bin/env bash
# =============================================================================
# create-cluster.sh — provision a GKE cluster for the dd remote-dev platform and
# install the SAME edge stack the Hetzner cluster uses (ingress-nginx + cert-manager
# + ArgoCD), so one Git repo/commit/branch deploys identically across AWS, Hetzner
# and GCP. The app layer is cloud-neutral; only the edge overlay differs per cloud
# (see remote/argocd/gateway-edge/).
#
# Why ingress-nginx + cert-manager instead of GKE-native Ingress/ManagedCerts:
# keeping the same controller + issuer on every cloud means the gateway-edge/gcp
# overlay is byte-for-byte the Hetzner overlay with a different host — no GKE-only
# objects leak into the shared layer.
#
# Prereqs:
#   gcloud auth login && gcloud config set project <PROJECT>
#   gcloud services enable container.googleapis.com compute.googleapis.com
#
# Usage:
#   ./create-cluster.sh                       # defaults below
#   PROJECT=my-proj REGION=us-central1 ./create-cluster.sh
# =============================================================================
set -euo pipefail

PROJECT="${PROJECT:-$(gcloud config get-value project 2>/dev/null)}"
REGION="${REGION:-us-central1}"
CLUSTER="${CLUSTER:-dd-k8s}"
# 3 nodes to mirror the Hetzner control-plane count; bump for heavier load.
NUM_NODES="${NUM_NODES:-3}"
MACHINE_TYPE="${MACHINE_TYPE:-e2-standard-8}"
CERT_MANAGER_VERSION="${CERT_MANAGER_VERSION:-v1.16.2}"
INGRESS_NGINX_CHART_VERSION="${INGRESS_NGINX_CHART_VERSION:-4.11.3}"
ACME_EMAIL="${ACME_EMAIL:-admin@oresoftware.com}"

[ -n "$PROJECT" ] || { echo "ERROR: set PROJECT or 'gcloud config set project'." >&2; exit 1; }
command -v gcloud >/dev/null || { echo "ERROR: gcloud not installed." >&2; exit 1; }

echo "==> Creating GKE cluster '${CLUSTER}' in ${REGION} (${NUM_NODES}x ${MACHINE_TYPE}) on ${PROJECT}"
if ! gcloud container clusters describe "$CLUSTER" --region "$REGION" --project "$PROJECT" >/dev/null 2>&1; then
  gcloud container clusters create "$CLUSTER" \
    --project "$PROJECT" --region "$REGION" \
    --num-nodes "$NUM_NODES" --machine-type "$MACHINE_TYPE" \
    --release-channel regular --enable-ip-alias \
    --addons HttpLoadBalancing
fi
gcloud container clusters get-credentials "$CLUSTER" --region "$REGION" --project "$PROJECT"

echo "==> cert-manager ${CERT_MANAGER_VERSION}"
kubectl apply --server-side --force-conflicts \
  -f "https://github.com/cert-manager/cert-manager/releases/download/${CERT_MANAGER_VERSION}/cert-manager.yaml"
kubectl -n cert-manager rollout status deploy/cert-manager --timeout=180s
kubectl -n cert-manager rollout status deploy/cert-manager-webhook --timeout=180s

echo "==> ingress-nginx ${INGRESS_NGINX_CHART_VERSION} (Service type LoadBalancer => GCP L4 LB)"
helm repo add ingress-nginx https://kubernetes.github.io/ingress-nginx >/dev/null 2>&1 || true
helm repo update >/dev/null 2>&1 || true
helm upgrade --install ingress-nginx ingress-nginx/ingress-nginx \
  --version "$INGRESS_NGINX_CHART_VERSION" -n ingress-nginx --create-namespace \
  --set controller.service.type=LoadBalancer
kubectl -n ingress-nginx rollout status deploy/ingress-nginx-controller --timeout=240s

echo "==> letsencrypt-prod ClusterIssuer (HTTP-01 via ingress-nginx) — same name as Hetzner"
kubectl apply -f - <<ISSUER
apiVersion: cert-manager.io/v1
kind: ClusterIssuer
metadata:
  name: letsencrypt-prod
spec:
  acme:
    server: https://acme-v02.api.letsencrypt.org/directory
    email: ${ACME_EMAIL}
    privateKeySecretRef:
      name: letsencrypt-prod-key
    solvers:
      - http01:
          ingress:
            ingressClassName: nginx
ISSUER

LB_IP="$(kubectl -n ingress-nginx get svc ingress-nginx-controller -o jsonpath='{.status.loadBalancer.ingress[0].ip}' 2>/dev/null || true)"
echo ""
echo "============================================================"
echo "  GKE cluster '${CLUSTER}' ready with ingress-nginx + cert-manager."
echo "  Ingress LB IP: ${LB_IP:-<pending — re-check svc ingress-nginx-controller>}"
echo "============================================================"
echo "  Next:"
echo "   1. Set the gateway host in remote/argocd/gateway-edge/overlays/gcp/ingress.yaml"
echo "      to hello.\${LB_IP//./-}.sslip.io  (LB_IP=${LB_IP:-?})."
echo "   2. Bootstrap ArgoCD here and point the gateway-edge Application at"
echo "      remote/argocd/gateway-edge/overlays/gcp (see that dir's README)."
echo "============================================================"
