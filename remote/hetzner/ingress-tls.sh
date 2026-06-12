#!/usr/bin/env bash
# =============================================================================
# ingress-tls.sh — add the L7/L4 entry layer to the WireGuard cluster:
#   * ingress-nginx as a DaemonSet on hostNetwork (every node IP serves 80/443
#     and the TCP port) — this IS the in-cluster load balancer / router. It is
#     independent of which node is the control-plane (CP is just a schedulable
#     node here), so a CP change never breaks data-path traffic.
#   * cert-manager + ClusterIssuers: selfsigned (active) plus Let's Encrypt
#     staging & prod (ready — flip to them once a real domain points here).
#   * hello-world behind an HTTPS Ingress (TLS terminated at the ingress).
#   * a raw TCP port (L4) forwarded via ingress-nginx tcp-services.
#
# HTTP routing = by Host/path (L7). TCP routing = by port (L4). Both ride the
# same DaemonSet bound to all node IPs.
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SSH_KEY="${SSH_KEY:-$HOME/.ssh/id_hetzner}"
FW_NAME="${FW_NAME:-dd-k8s-fw}"
EMAIL="${EMAIL:-alexander.d.mills@gmail.com}"
CERT_MANAGER_VERSION="${CERT_MANAGER_VERSION:-v1.16.2}"
INGRESS_NGINX_CHART_VERSION="${INGRESS_NGINX_CHART_VERSION:-4.11.3}"
TCP_DEMO_PORT="${TCP_DEMO_PORT:-9000}"
ACTIVE_ISSUER="${ACTIVE_ISSUER:-selfsigned}"   # selfsigned | letsencrypt-staging | letsencrypt-prod

SSH_OPTS=(-i "$SSH_KEY" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15)
rssh() { local ip="$1"; shift; ssh "${SSH_OPTS[@]}" root@"$ip" "$@"; }

command -v hcloud >/dev/null || { echo "ERROR: hcloud not authenticated." >&2; exit 1; }
CP_NODE="${CP_NODE:-$(hcloud server list --selector role=dd-k8s -o noheader -o columns=name | sort | head -1)}"
CP_IP="$(hcloud server ip "$CP_NODE")"
[ -n "$CP_IP" ] || { echo "ERROR: cannot resolve control-plane node IP." >&2; exit 1; }
ssh-keygen -R "$CP_IP" >/dev/null 2>&1 || true   # Hetzner reuses IPs
HOST="hello.${CP_IP//./-}.sslip.io"   # sslip.io resolves this to CP_IP — no real DNS needed
echo "==> control-plane ${CP_IP}   demo host ${HOST}   active issuer ${ACTIVE_ISSUER}"

# ---- firewall: open 80/443 + TCP demo port (public; LE HTTP-01 needs 80) ----
existing="$(hcloud firewall describe "$FW_NAME" -o json 2>/dev/null | jq -r '.rules[]?|select(.direction=="in")|.port' 2>/dev/null || true)"
ADMIN_IP="$(curl -4 -fsS https://ifconfig.me 2>/dev/null || true)"
# 80/443 public (web + LE http-01); the raw TCP demo port is admin-only.
for p in 80 443; do
  grep -qx "$p" <<<"$existing" || hcloud firewall add-rule "$FW_NAME" --direction in --protocol tcp --port "$p" \
    --source-ips 0.0.0.0/0 --source-ips ::/0 --description "ingress ${p}"
done
if ! grep -qx "$TCP_DEMO_PORT" <<<"$existing"; then
  if [ -n "$ADMIN_IP" ]; then
    hcloud firewall add-rule "$FW_NAME" --direction in --protocol tcp --port "$TCP_DEMO_PORT" \
      --source-ips "${ADMIN_IP}/32" --description "TCP demo (admin only)"
  else
    echo "WARNING: no IPv4 detected; leaving TCP ${TCP_DEMO_PORT} unopened." >&2
  fi
fi

# ---- everything else runs on the control plane (helm/kubectl live there) ----
rssh "$CP_IP" "EMAIL='${EMAIL}' HOST='${HOST}' CMV='${CERT_MANAGER_VERSION}' INGV='${INGRESS_NGINX_CHART_VERSION}' TCPP='${TCP_DEMO_PORT}' ACTIVE='${ACTIVE_ISSUER}' bash -s" <<'REMOTE'
set -euxo pipefail
export KUBECONFIG=/etc/kubernetes/admin.conf

# --- cert-manager (server-side apply: CRDs exceed the client-side annotation limit) ---
kubectl apply --server-side --force-conflicts -f "https://github.com/cert-manager/cert-manager/releases/download/${CMV}/cert-manager.yaml"
kubectl -n cert-manager rollout status deploy/cert-manager --timeout=180s
kubectl -n cert-manager rollout status deploy/cert-manager-webhook --timeout=180s

# --- ingress-nginx: DaemonSet on hostNetwork => all node IPs serve 80/443/TCP ---
helm repo add ingress-nginx https://kubernetes.github.io/ingress-nginx >/dev/null 2>&1 || true
helm repo update >/dev/null
cat >/tmp/ingress-values.yaml <<VALUES
controller:
  kind: DaemonSet
  hostNetwork: true
  dnsPolicy: ClusterFirstWithHostNet
  service:
    type: ClusterIP
  ingressClassResource:
    default: true
  config:
    ssl-protocols: "TLSv1.2 TLSv1.3"
    ssl-ciphers: "ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384:ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-CHACHA20-POLY1305"
    ssl-redirect: "true"
    hsts: "true"
    hsts-max-age: "31536000"
    server-tokens: "false"
    hide-headers: "Server,X-Powered-By"
tcp:
  "${TCPP}": "default/hello-world:80"
VALUES
helm upgrade --install ingress-nginx ingress-nginx/ingress-nginx \
  --version "${INGV}" -n ingress-nginx --create-namespace -f /tmp/ingress-values.yaml
kubectl -n ingress-nginx rollout status ds/ingress-nginx-controller --timeout=240s

# --- ClusterIssuers: selfsigned (active) + Let's Encrypt staging & prod (ready) ---
cat >/tmp/issuers.yaml <<ISSUERS
apiVersion: cert-manager.io/v1
kind: ClusterIssuer
metadata:
  name: selfsigned
spec:
  selfSigned: {}
---
apiVersion: cert-manager.io/v1
kind: ClusterIssuer
metadata:
  name: letsencrypt-staging
spec:
  acme:
    server: https://acme-staging-v02.api.letsencrypt.org/directory
    email: ${EMAIL}
    privateKeySecretRef:
      name: letsencrypt-staging-key
    solvers:
      - http01:
          ingress:
            ingressClassName: nginx
---
apiVersion: cert-manager.io/v1
kind: ClusterIssuer
metadata:
  name: letsencrypt-prod
spec:
  acme:
    server: https://acme-v02.api.letsencrypt.org/directory
    email: ${EMAIL}
    privateKeySecretRef:
      name: letsencrypt-prod-key
    solvers:
      - http01:
          ingress:
            ingressClassName: nginx
ISSUERS
# webhook can lag readiness — retry the issuer apply
for i in 1 2 3 4 5 6; do kubectl apply -f /tmp/issuers.yaml && break || { echo "retry issuers ($i)"; sleep 10; }; done

# --- HTTPS Ingress for hello-world; cert from the active issuer; TLS at ingress ---
cat >/tmp/hello-ingress.yaml <<INGRESS
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: hello-world
  annotations:
    cert-manager.io/cluster-issuer: "${ACTIVE}"
spec:
  ingressClassName: nginx
  tls:
    - hosts: ["${HOST}"]
      secretName: hello-world-tls
  rules:
    - host: "${HOST}"
      http:
        paths:
          - path: /
            pathType: Prefix
            backend:
              service:
                name: hello-world
                port:
                  number: 80
INGRESS
kubectl apply -f /tmp/hello-ingress.yaml

# --- wait for the cert to be issued ---
for _ in $(seq 1 45); do
  if [ "$(kubectl get certificate hello-world -o jsonpath='{.status.conditions[?(@.type=="Ready")].status}' 2>/dev/null)" = "True" ]; then break; fi
  sleep 4
done
echo "--- issuers ---";  kubectl get clusterissuer
echo "--- cert ---";     kubectl get certificate hello-world 2>/dev/null || true
echo "--- ingress ---";  kubectl get ingress hello-world
echo "--- tcp cm ---";   kubectl -n ingress-nginx get cm ingress-nginx-tcp -o jsonpath='{.data}' 2>/dev/null; echo
REMOTE

echo ""
echo "============================================================"
echo "  Ingress + TLS layer up"
echo "============================================================"
echo "  HTTPS (L7, TLS terminated at ingress, ${ACTIVE_ISSUER}):"
echo "    https://${HOST}/            # -k while self-signed/staging"
echo "  Raw TCP (L4 via ingress-nginx tcp-services):"
echo "    ${CP_IP}:${TCP_DEMO_PORT}  ->  hello-world:80   (any node IP works)"
echo ""
echo "  Switch to TRUSTED Let's Encrypt once a real domain points here:"
echo "    1) DNS A-record <your-host> -> ${CP_IP} (or all node IPs)"
echo "    2) edit ingress host+tls to <your-host>, then:"
echo "       kubectl annotate ingress hello-world cert-manager.io/cluster-issuer=letsencrypt-prod --overwrite"
echo "============================================================"
