#!/usr/bin/env bash
# =============================================================================
# apply-gateway.sh — (re)apply the Hetzner entry gateway + its public Ingress.
#
# Idempotent. Resolves the CURRENT control-plane IP from hcloud (Hetzner reuses
# IPs on rebuild, so never hard-code it), derives the sslip host, and applies:
#   * dd-remote-gateway-hz Deployment + Service (reuses dd-remote-gateway-config)
#   * the public Ingress (host = hello.<cp-ip>.sslip.io), cert via cert-manager
#
# Prereq: ingress-tls.sh has run (ingress-nginx + cert-manager + ClusterIssuers),
# the dd-remote-gateway-config ConfigMap + dd-remote-gateway-tls Secret exist,
# and the nginx image is present (it's the public docker.io/library/nginx).
# =============================================================================
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SSH_KEY="${SSH_KEY:-$HOME/.ssh/id_hetzner}"
SSH_OPTS=(-i "$SSH_KEY" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15)

command -v hcloud >/dev/null || { echo "ERROR: hcloud not authenticated." >&2; exit 1; }
command -v envsubst >/dev/null || { echo "ERROR: envsubst (gettext) required." >&2; exit 1; }

CP_NODE="${CP_NODE:-$(hcloud server list --selector role=dd-k8s -o noheader -o columns=name | sort | head -1)}"
CP_IP="$(hcloud server ip "$CP_NODE")"
[ -n "$CP_IP" ] || { echo "ERROR: cannot resolve control-plane IP." >&2; exit 1; }
export HETZNER_INGRESS_HOST="hello.${CP_IP//./-}.sslip.io"
echo "==> control-plane ${CP_NODE} ${CP_IP}   host ${HETZNER_INGRESS_HOST}"

ssh-keygen -R "$CP_IP" >/dev/null 2>&1 || true
# Join as a multi-doc stream — explicit `---` separators between each manifest.
RENDERED="$(cat "$SCRIPT_DIR/dd-remote-gateway-hz.deployment.yaml"; echo '---'; \
            cat "$SCRIPT_DIR/dd-remote-gateway-hz.service.yaml"; echo '---'; \
            envsubst '${HETZNER_INGRESS_HOST}' < "$SCRIPT_DIR/dd-remote-gateway-hz.ingress.yaml")"

ssh "${SSH_OPTS[@]}" root@"$CP_IP" 'KUBECONFIG=/etc/kubernetes/admin.conf kubectl apply -f -' <<<"$RENDERED"
echo "==> applied. Verify:  curl -k https://${HETZNER_INGRESS_HOST}/home"
