#!/usr/bin/env bash
set -euo pipefail

CERT_NAME="${CERT_NAME:-98.90.186.114}"
CERTBOT_BIN="${CERTBOT_BIN:-/home/ec2-user/certbot-venv-312/bin/certbot}"
CERTBOT_CONFIG_DIR="${CERTBOT_CONFIG_DIR:-/home/ec2-user/letsencrypt/config}"
CERTBOT_WORK_DIR="${CERTBOT_WORK_DIR:-/home/ec2-user/letsencrypt/work}"
CERTBOT_LOGS_DIR="${CERTBOT_LOGS_DIR:-/home/ec2-user/letsencrypt/logs}"
K8S_NAMESPACE="${K8S_NAMESPACE:-default}"
K8S_SECRET_NAME="${K8S_SECRET_NAME:-dd-remote-gateway-tls}"
K8S_GATEWAY_DEPLOYMENT="${K8S_GATEWAY_DEPLOYMENT:-dd-remote-gateway}"

deploy_gateway_secret() {
  kubectl create secret tls "${K8S_SECRET_NAME}" \
    --cert="${CERTBOT_CONFIG_DIR}/live/${CERT_NAME}/fullchain.pem" \
    --key="${CERTBOT_CONFIG_DIR}/live/${CERT_NAME}/privkey.pem" \
    -n "${K8S_NAMESPACE}" \
    --dry-run=client -o yaml | kubectl apply -f -

  kubectl rollout restart "deployment/${K8S_GATEWAY_DEPLOYMENT}" -n "${K8S_NAMESPACE}"
  kubectl rollout status "deployment/${K8S_GATEWAY_DEPLOYMENT}" -n "${K8S_NAMESPACE}" --timeout=180s
}

renew_certificate() {
  "${CERTBOT_BIN}" renew \
    --config-dir "${CERTBOT_CONFIG_DIR}" \
    --work-dir "${CERTBOT_WORK_DIR}" \
    --logs-dir "${CERTBOT_LOGS_DIR}" \
    --deploy-hook "$0 deploy"
}

case "${1:-renew}" in
  deploy)
    deploy_gateway_secret
    ;;
  renew)
    renew_certificate
    ;;
  *)
    echo "usage: $0 [renew|deploy]" >&2
    exit 64
    ;;
esac
