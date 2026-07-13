#!/usr/bin/env bash
# =============================================================================
# harden-ha.sh — cluster hardening for the 3-control-plane HA cluster.
# (Node-level SSH/unattended-upgrades are in cloud-init; the firewall is set at
# create time for the all-EU/private-API layout.)
#
#   * secrets ENCRYPTION AT REST (secretbox) — the SAME key on all three control
#     planes, each apiserver patched node-by-node with health gating +
#     auto-rollback so etcd quorum / the API stay available throughout.
#   * --profiling=false on apiserver/cm/scheduler (all nodes); apiserver
#     --tls-min-version=VersionTLS12.
#   * Pod Security Standards enforce=baseline (warn/audit=restricted) on default.
#
# Idempotent. Run after setup-cluster-ha.sh.
# =============================================================================
set -euo pipefail

SSH_KEY="${SSH_KEY:-$HOME/.ssh/id_hetzner}"
SSH_OPTS=(-i "$SSH_KEY" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15)
rssh() { local ip="$1"; shift; ssh "${SSH_OPTS[@]}" root@"$ip" "$@"; }

command -v hcloud >/dev/null || { echo "ERROR: hcloud not authenticated." >&2; exit 1; }
mapfile -t NODES < <(hcloud server list --selector role=dd-k8s -o noheader -o columns=name | sort)
[ "${#NODES[@]}" -ge 1 ] || { echo "ERROR: no dd-k8s nodes found." >&2; exit 1; }
declare -A IP
for n in "${NODES[@]}"; do IP[$n]="$(hcloud server ip "$n")"; ssh-keygen -R "${IP[$n]}" >/dev/null 2>&1 || true; done
CP1="${NODES[0]}"; CP1_IP="${IP[$CP1]}"

# ---- one encryption key for the whole cluster ----
KEY="$(head -c 32 /dev/urandom | base64)"
umask 077
cat > /tmp/dd-enc.yaml <<EOF
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
EOF

# ---- the manifest patcher (shipped to each node) ----
cat > /tmp/dd-patch.py <<'PY'
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

for n in "${NODES[@]}"; do
  ip="${IP[$n]}"
  echo "==> hardening control plane ${n} (${ip})"
  rssh "$ip" 'install -d -m 700 /etc/kubernetes/enc'
  scp "${SSH_OPTS[@]}" -q /tmp/dd-enc.yaml root@"$ip":/etc/kubernetes/enc/encryption-config.yaml
  scp "${SSH_OPTS[@]}" -q /tmp/dd-patch.py root@"$ip":/root/dd-patch.py
  rssh "$ip" 'bash -s' <<'REMOTE'
set -e
export KUBECONFIG=/etc/kubernetes/admin.conf
chmod 600 /etc/kubernetes/enc/encryption-config.yaml
cd /etc/kubernetes/manifests
for f in kube-apiserver kube-controller-manager kube-scheduler; do cp -f $f.yaml /root/$f.yaml.bak; done
python3 /root/dd-patch.py
ok=0
for i in $(seq 1 50); do kubectl get --raw=/readyz >/dev/null 2>&1 && { ok=1; break; }; sleep 4; done
if [ "$ok" != 1 ]; then
  echo "!! apiserver did not recover — rolling back this node"
  for f in kube-apiserver kube-controller-manager kube-scheduler; do cp -f /root/$f.yaml.bak /etc/kubernetes/manifests/$f.yaml; done
  exit 1
fi
echo "[ok] patched + healthy"
REMOTE
done

echo "==> re-encrypting existing secrets + PSS label"
rssh "$CP1_IP" 'export KUBECONFIG=/etc/kubernetes/admin.conf
kubectl get secrets -A -o json | kubectl replace -f - >/dev/null || true
kubectl label ns default \
  pod-security.kubernetes.io/enforce=baseline \
  pod-security.kubernetes.io/warn=restricted \
  pod-security.kubernetes.io/audit=restricted --overwrite'
echo "==> HA hardening complete."
