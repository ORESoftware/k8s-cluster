#!/usr/bin/env bash
set -euo pipefail

# CERT_NAME is only a fallback for manual `deploy` runs. When certbot invokes the
# deploy/renew hook it exports RENEWED_LINEAGE (the live/<name> dir of the cert that
# just renewed), which we always prefer — that way the hook tracks the current cert
# even after the EC2 public IP (and thus the LE IP-cert lineage name) changes. The
# old breakage was a stale lineage name hard-coded here (54.91.17.58).
CERT_NAME="${CERT_NAME:-98.90.186.114}"
CERTBOT_BIN="${CERTBOT_BIN:-/home/ec2-user/certbot-venv-312/bin/certbot}"
CERTBOT_CONFIG_DIR="${CERTBOT_CONFIG_DIR:-/home/ec2-user/letsencrypt/config}"
CERTBOT_WORK_DIR="${CERTBOT_WORK_DIR:-/home/ec2-user/letsencrypt/work}"
CERTBOT_LOGS_DIR="${CERTBOT_LOGS_DIR:-/home/ec2-user/letsencrypt/logs}"
K8S_NAMESPACE="${K8S_NAMESPACE:-default}"
K8S_SECRET_NAME="${K8S_SECRET_NAME:-dd-remote-gateway-tls}"
K8S_GATEWAY_DEPLOYMENT="${K8S_GATEWAY_DEPLOYMENT:-dd-remote-gateway}"
KUBECTL_BIN="${KUBECTL_BIN:-kubectl}"
KUBECTL_KUBECONFIG="${KUBECTL_KUBECONFIG:-/etc/kubernetes/admin.conf}"
KUBECTL_SUDO="${KUBECTL_SUDO:-sudo}"

kubectl_cmd() {
  local cmd=()
  if [ -n "${KUBECTL_SUDO}" ]; then
    cmd+=("${KUBECTL_SUDO}")
  fi
  cmd+=("${KUBECTL_BIN}")
  if [ -n "${KUBECTL_KUBECONFIG}" ]; then
    cmd+=(--kubeconfig "${KUBECTL_KUBECONFIG}")
  fi
  "${cmd[@]}" "$@"
}

deploy_gateway_secret() {
  # Prefer the lineage certbot just renewed; fall back to CERT_NAME for manual runs.
  local lineage="${RENEWED_LINEAGE:-${CERTBOT_CONFIG_DIR}/live/${CERT_NAME}}"
  if [ ! -f "${lineage}/fullchain.pem" ]; then
    echo "ERROR: no fullchain.pem under ${lineage} (RENEWED_LINEAGE=${RENEWED_LINEAGE:-unset}, CERT_NAME=${CERT_NAME})" >&2
    exit 1
  fi

  kubectl_cmd create secret tls "${K8S_SECRET_NAME}" \
    --cert="${lineage}/fullchain.pem" \
    --key="${lineage}/privkey.pem" \
    -n "${K8S_NAMESPACE}" \
    --dry-run=client -o yaml | kubectl_cmd apply --validate=false -f -

  kubectl_cmd rollout restart "deployment/${K8S_GATEWAY_DEPLOYMENT}" -n "${K8S_NAMESPACE}"
  kubectl_cmd rollout status "deployment/${K8S_GATEWAY_DEPLOYMENT}" -n "${K8S_NAMESPACE}" --timeout=180s
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
