#!/usr/bin/env bash
# =============================================================================
# harden.sh — post-setup hardening pass for the Hetzner cluster. Idempotent.
#
# Applies the cluster- and firewall-level hardening (the node-level bits —
# key-only SSH + unattended-upgrades — are baked into cloud-init.yaml):
#   * Firewall: WireGuard restricted to the node IPs; SSH/API/ArgoCD/NodePort/
#     TCP-demo to the admin IP only; HTTP/HTTPS public.
#   * Secrets ENCRYPTION AT REST (secretbox) + re-encrypt existing secrets.
#   * kube-apiserver/controller-manager/scheduler --profiling=false,
#     apiserver --tls-min-version=VersionTLS12. Auto-rollback if apiserver
#     fails to recover.
#   * Pod Security Standards: enforce=baseline (warn/audit=restricted) on the
#     default (app) namespace.
#
# Run after setup-cluster.sh. Re-runnable.
# =============================================================================
set -euo pipefail

SSH_KEY="${SSH_KEY:-$HOME/.ssh/id_hetzner}"
FW_NAME="${FW_NAME:-dd-k8s-fw}"
LOCS=(${LOCS:-ash hil fsn1})
CP_LOC="${CP_LOC:-ash}"
SSH_OPTS=(-i "$SSH_KEY" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15)
rssh() { local ip="$1"; shift; ssh "${SSH_OPTS[@]}" root@"$ip" "$@"; }

command -v hcloud >/dev/null || { echo "ERROR: hcloud not authenticated." >&2; exit 1; }
CP_IP="$(hcloud server ip "dd-k8s-${CP_LOC}")"

# ---- firewall: least-privilege rule set ----
ADMIN_IP="$(curl -4 -fsS https://ifconfig.me 2>/dev/null || curl -4 -fsS https://api.ipify.org 2>/dev/null || true)"
if [ -n "$ADMIN_IP" ]; then
  node_cidrs=()
  for loc in "${LOCS[@]}"; do node_cidrs+=("\"$(hcloud server ip "dd-k8s-${loc}")/32\""); done
  wg_sources="$(IFS=,; echo "${node_cidrs[*]}")"
  cat >/tmp/dd-fw-rules.json <<JSON
[
  {"direction":"in","protocol":"udp","port":"51820","source_ips":[${wg_sources}],"description":"WireGuard mesh (node-to-node only)"},
  {"direction":"in","protocol":"tcp","port":"22","source_ips":["${ADMIN_IP}/32"],"description":"admin SSH"},
  {"direction":"in","protocol":"tcp","port":"6443","source_ips":["${ADMIN_IP}/32"],"description":"admin kube-API"},
  {"direction":"in","protocol":"tcp","port":"30443","source_ips":["${ADMIN_IP}/32"],"description":"admin ArgoCD"},
  {"direction":"in","protocol":"tcp","port":"30080","source_ips":["${ADMIN_IP}/32"],"description":"admin hello-world NodePort"},
  {"direction":"in","protocol":"tcp","port":"9000","source_ips":["${ADMIN_IP}/32"],"description":"admin TCP demo"},
  {"direction":"in","protocol":"tcp","port":"80","source_ips":["0.0.0.0/0","::/0"],"description":"ingress HTTP (LE http-01 + redirect)"},
  {"direction":"in","protocol":"tcp","port":"443","source_ips":["0.0.0.0/0","::/0"],"description":"ingress HTTPS"}
]
JSON
  echo "==> Tightening firewall ${FW_NAME} (WG node-only; mgmt/demo admin-only @ ${ADMIN_IP})"
  hcloud firewall replace-rules "$FW_NAME" --rules-file /tmp/dd-fw-rules.json >/dev/null
else
  echo "WARNING: no public IPv4 detected — leaving firewall as-is to avoid lockout." >&2
fi

# ---- cluster hardening on the control plane ----
echo "==> Encryption-at-rest + profiling=false + PSS on ${CP_LOC} (${CP_IP})"
rssh "$CP_IP" 'bash -s' <<'HARDEN'
set -e
export KUBECONFIG=/etc/kubernetes/admin.conf
cd /etc/kubernetes/manifests
for f in kube-apiserver kube-controller-manager kube-scheduler; do cp -f $f.yaml /root/$f.yaml.bak; done

install -d -m 700 /etc/kubernetes/enc
if [ ! -f /etc/kubernetes/enc/encryption-config.yaml ]; then
  KEY=$(head -c 32 /dev/urandom | base64 -w0); umask 077
  cat >/etc/kubernetes/enc/encryption-config.yaml <<EC
apiVersion: apiserver.config.k8s.io/v1
kind: EncryptionConfiguration
resources:
  - resources: ["secrets"]
    providers:
      - secretbox:
          keys:
            - name: key1
              secret: ${KEY}
      - identity: {}
EC
fi

python3 - <<'PY'
api='/etc/kubernetes/manifests/kube-apiserver.yaml'
L=open(api).read().split('\n')
def has(s): return any(s in l for l in L)
out=[]
for l in L:
    out.append(l)
    if l=='    - kube-apiserver' and not has('--encryption-provider-config'):
        out+=['    - --encryption-provider-config=/etc/kubernetes/enc/encryption-config.yaml',
              '    - --profiling=false','    - --tls-min-version=VersionTLS12']
    if l=='    volumeMounts:' and not has('mountPath: /etc/kubernetes/enc'):
        out+=['    - mountPath: /etc/kubernetes/enc','      name: enc','      readOnly: true']
    if l=='  volumes:' and not has('path: /etc/kubernetes/enc'):
        out+=['  - hostPath:','      path: /etc/kubernetes/enc','      type: DirectoryOrCreate','    name: enc']
open(api,'w').write('\n'.join(out))
for f,a in [('kube-controller-manager','    - kube-controller-manager'),('kube-scheduler','    - kube-scheduler')]:
    p='/etc/kubernetes/manifests/%s.yaml'%f; M=open(p).read().split('\n')
    if not any('--profiling=false' in x for x in M):
        o=[]
        for x in M:
            o.append(x)
            if x==a: o.append('    - --profiling=false')
        open(p,'w').write('\n'.join(o))
PY

ok=0
for i in $(seq 1 50); do kubectl get --raw=/readyz >/dev/null 2>&1 && { ok=1; break; }; sleep 4; done
if [ "$ok" != 1 ]; then
  echo "!! apiserver did not recover — rolling back"
  for f in kube-apiserver kube-controller-manager kube-scheduler; do cp -f /root/$f.yaml.bak /etc/kubernetes/manifests/$f.yaml; done
  exit 1
fi
kubectl get secrets -A -o json | kubectl replace -f - >/dev/null || true
kubectl label ns default \
  pod-security.kubernetes.io/enforce=baseline \
  pod-security.kubernetes.io/warn=restricted \
  pod-security.kubernetes.io/audit=restricted --overwrite
echo "[ok] encryption-at-rest active, profiling off, PSS baseline on default ns"
HARDEN

echo "==> Hardening complete."
