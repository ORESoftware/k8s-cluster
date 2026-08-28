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
if [ -n "${K8S_GATEWAY_WORKLOAD:-}" ]; then
  GATEWAY_WORKLOAD="${K8S_GATEWAY_WORKLOAD}"
elif [ -n "${K8S_GATEWAY_DEPLOYMENT:-}" ]; then
  # Backward compatibility for operator overrides from before the gateway was
  # migrated from a Deployment to the Git-desired DaemonSet.
  GATEWAY_WORKLOAD="deployment/${K8S_GATEWAY_DEPLOYMENT}"
else
  GATEWAY_WORKLOAD="daemonset/dd-remote-gateway"
fi
KUBECTL_BIN="${KUBECTL_BIN:-kubectl}"
KUBECTL_KUBECONFIG="${KUBECTL_KUBECONFIG:-/etc/kubernetes/admin.conf}"
KUBECTL_SUDO="${KUBECTL_SUDO:-sudo}"

# Keep call sites readable while still making every Kubernetes operation use the
# reviewed executable, optional privilege boundary, and explicit kubeconfig.
# `command` prevents the default KUBECTL_BIN=kubectl from recursively invoking
# this wrapper function.
kubectl() {
  local cmd=()
  if [ -n "${KUBECTL_SUDO}" ]; then
    cmd+=("${KUBECTL_SUDO}")
  fi
  cmd+=("${KUBECTL_BIN}")
  if [ -n "${KUBECTL_KUBECONFIG}" ]; then
    cmd+=(--kubeconfig "${KUBECTL_KUBECONFIG}")
  fi
  command "${cmd[@]}" "$@"
}

create_gateway_secret_manifest() {
  if [ -n "${RENEWED_LINEAGE:-}" ]; then
    kubectl create secret tls "${K8S_SECRET_NAME}" \
      --cert="${RENEWED_LINEAGE}/fullchain.pem" \
      --key="${RENEWED_LINEAGE}/privkey.pem" \
      -n "${K8S_NAMESPACE}" \
      --dry-run=client -o yaml
  else
    kubectl create secret tls "${K8S_SECRET_NAME}" \
      --cert="${CERTBOT_CONFIG_DIR}/live/${CERT_NAME}/fullchain.pem" \
      --key="${CERTBOT_CONFIG_DIR}/live/${CERT_NAME}/privkey.pem" \
      -n "${K8S_NAMESPACE}" \
      --dry-run=client -o yaml
  fi
}

deploy_gateway_secret() {
  # Prefer the lineage certbot just renewed; fall back to CERT_NAME for manual runs.
  local lineage="${RENEWED_LINEAGE:-${CERTBOT_CONFIG_DIR}/live/${CERT_NAME}}"
  if [ ! -f "${lineage}/fullchain.pem" ] || [ ! -f "${lineage}/privkey.pem" ]; then
    echo "ERROR: incomplete certificate lineage under ${lineage} (RENEWED_LINEAGE=${RENEWED_LINEAGE:-unset}, CERT_NAME=${CERT_NAME})" >&2
    exit 1
  fi

  create_gateway_secret_manifest | kubectl apply --validate=false -f -

  case "${GATEWAY_WORKLOAD}" in
    deployment/*|daemonset/*) ;;
    *)
      echo "ERROR: K8S_GATEWAY_WORKLOAD must be deployment/NAME or daemonset/NAME" >&2
      exit 64
      ;;
  esac

  kubectl rollout restart "${GATEWAY_WORKLOAD}" -n "${K8S_NAMESPACE}"
  kubectl rollout status "${GATEWAY_WORKLOAD}" -n "${K8S_NAMESPACE}" --timeout=180s
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
