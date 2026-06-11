#!/usr/bin/env bash
# =============================================================================
# setup-cluster.sh — form ONE K8s cluster across the 3 Hetzner boxes, wired by
# a WireGuard mesh that carries all cluster traffic.
#
#   control-plane : ash   (10.10.0.1)   etcd local — never spans regions
#   worker        : hil   (10.10.0.2)
#   worker        : fsn1  (10.10.0.3)
#
# kubelet advertises each node's WireGuard IP (--node-ip), the API server
# advertises 10.10.0.1, and Cilium tunnels pod traffic to peer node-IPs — so
# every hop (kubelet->API, etcd, pod->pod) rides the encrypted wg0 tunnel.
#
# Run AFTER create-cluster.sh, once cloud-init prereqs have finished.
# Idempotent: safe to re-run.
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SSH_KEY="${SSH_KEY:-$HOME/.ssh/id_hetzner}"
WG_PORT="${WG_PORT:-51820}"
POD_CIDR="${POD_CIDR:-10.244.0.0/16}"
HELLO="$SCRIPT_DIR/hello-world.yaml"

# Control-plane FIRST. WG IPs are fixed per location.
LOCS=(ash hil fsn1)
CP_LOC="ash"
declare -A WGIP=( [ash]="10.10.0.1" [hil]="10.10.0.2" [fsn1]="10.10.0.3" )

SSH_OPTS=(-i "$SSH_KEY" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 -o ServerAliveInterval=15)
rssh() { local ip="$1"; shift; ssh "${SSH_OPTS[@]}" root@"$ip" "$@"; }

command -v hcloud >/dev/null || { echo "ERROR: hcloud not installed/authenticated." >&2; exit 1; }
[ -f "$SSH_KEY" ] || { echo "ERROR: SSH key $SSH_KEY not found." >&2; exit 1; }

# ---- resolve public IPv4s ----
declare -A PUBIP
for loc in "${LOCS[@]}"; do
  ip="$(hcloud server ip "dd-k8s-${loc}" 2>/dev/null || true)"
  [ -n "$ip" ] || { echo "ERROR: no IPv4 for dd-k8s-${loc} — run create-cluster.sh first." >&2; exit 1; }
  PUBIP[$loc]="$ip"
  echo "  dd-k8s-${loc}: public ${ip}  wg ${WGIP[$loc]}"
done

# ---- wait for cloud-init prereqs on every node ----
for loc in "${LOCS[@]}"; do
  ip="${PUBIP[$loc]}"
  echo "==> Waiting for prereqs on ${loc} (${ip})"
  for _ in $(seq 1 90); do
    rssh "$ip" 'test -f /opt/dd/prereqs-complete' 2>/dev/null && break
    sleep 10
  done
  rssh "$ip" 'test -f /opt/dd/prereqs-complete' \
    || { echo "ERROR: prereqs never completed on ${loc}; check /var/log/dd-prereqs.log" >&2; exit 1; }
done

# ---- collect WireGuard public keys ----
declare -A WGPUB
for loc in "${LOCS[@]}"; do
  WGPUB[$loc]="$(rssh "${PUBIP[$loc]}" 'cat /opt/dd/wg-pubkey')"
done

# ---- configure the full WireGuard mesh ----
for loc in "${LOCS[@]}"; do
  peers=""
  for other in "${LOCS[@]}"; do
    [ "$other" = "$loc" ] && continue
    peers+="${WGPUB[$other]}|${WGIP[$other]}|${PUBIP[$other]} "
  done
  echo "==> WireGuard on ${loc} (${WGIP[$loc]})"
  rssh "${PUBIP[$loc]}" "SELF_WG='${WGIP[$loc]}' WG_PORT='${WG_PORT}' PEERS='${peers}' bash -s" <<'REMOTE'
set -euo pipefail
priv="$(cat /etc/wireguard/privatekey)"
{
  printf '[Interface]\nAddress = %s/24\nListenPort = %s\nPrivateKey = %s\n' "$SELF_WG" "$WG_PORT" "$priv"
  for p in $PEERS; do
    IFS='|' read -r pk wg ep <<<"$p"
    printf '\n[Peer]\nPublicKey = %s\nAllowedIPs = %s/32\nEndpoint = %s:%s\nPersistentKeepalive = 25\n' "$pk" "$wg" "$ep" "$WG_PORT"
  done
} > /etc/wireguard/wg0.conf
chmod 600 /etc/wireguard/wg0.conf
systemctl enable wg-quick@wg0 >/dev/null 2>&1 || true
systemctl restart wg-quick@wg0
REMOTE
done

# ---- verify mesh from the control plane ----
echo "==> Verifying WireGuard mesh from ${CP_LOC}"
for other in "${LOCS[@]}"; do
  [ "$other" = "$CP_LOC" ] && continue
  rssh "${PUBIP[$CP_LOC]}" "ping -c2 -W3 ${WGIP[$other]} >/dev/null" \
    || { echo "ERROR: no WireGuard connectivity ${CP_LOC} -> ${other}" >&2; exit 1; }
  echo "    ${CP_LOC} -> ${other} (${WGIP[$other]}) OK"
done

# ---- control plane: kubeadm init + Cilium + storage + ArgoCD ----
CP_IP="${PUBIP[$CP_LOC]}"
echo "==> Bootstrapping control plane on ${CP_LOC}"
rssh "$CP_IP" "CP_WG='${WGIP[$CP_LOC]}' CP_PUB='${CP_IP}' POD_CIDR='${POD_CIDR}' bash -s" <<'REMOTE'
set -euxo pipefail
echo "KUBELET_EXTRA_ARGS=--node-ip=${CP_WG}" > /etc/default/kubelet

if [ ! -f /etc/kubernetes/admin.conf ]; then
  kubeadm init \
    --apiserver-advertise-address="${CP_WG}" \
    --apiserver-cert-extra-sans="${CP_PUB}" \
    --pod-network-cidr="${POD_CIDR}" \
    --skip-phases=addon/kube-proxy \
    --cri-socket=unix:///run/containerd/containerd.sock | tee /var/log/kubeadm-init.log
fi
mkdir -p /root/.kube && cp -f /etc/kubernetes/admin.conf /root/.kube/config
export KUBECONFIG=/etc/kubernetes/admin.conf

# small cluster: schedule workloads on the control-plane node too
kubectl taint nodes --all node-role.kubernetes.io/control-plane- 2>/dev/null || true

if ! cilium status >/dev/null 2>&1; then
  cilium install \
    --set kubeProxyReplacement=true \
    --set ipam.mode=kubernetes \
    --set k8sServiceHost="${CP_WG}" \
    --set k8sServicePort=6443
fi
cilium status --wait --wait-duration 5m || echo "(cilium still settling)"

LP_TAG="$(curl -fsSL https://api.github.com/repos/rancher/local-path-provisioner/releases/latest | jq -r .tag_name)"
kubectl apply -f "https://raw.githubusercontent.com/rancher/local-path-provisioner/${LP_TAG}/deploy/local-path-storage.yaml"
kubectl apply -f - <<'SC'
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: gp3
  annotations:
    storageclass.kubernetes.io/is-default-class: "true"
provisioner: rancher.io/local-path
volumeBindingMode: WaitForFirstConsumer
reclaimPolicy: Delete
SC

ARGOCD_TAG="$(curl -fsSL https://api.github.com/repos/argoproj/argo-cd/releases/latest | jq -r .tag_name)"
kubectl create namespace argocd 2>/dev/null || true
kubectl apply -n argocd -f "https://raw.githubusercontent.com/argoproj/argo-cd/${ARGOCD_TAG}/manifests/install.yaml"
kubectl -n argocd rollout status deploy/argocd-server --timeout=300s || true
kubectl -n argocd patch svc argocd-server -p '{"spec":{"type":"NodePort","ports":[{"name":"https","port":443,"targetPort":8080,"nodePort":30443}]}}' || true
REMOTE

# ---- join workers over the WG tunnel ----
JOIN="$(rssh "$CP_IP" 'kubeadm token create --print-join-command') --cri-socket=unix:///run/containerd/containerd.sock"
for loc in "${LOCS[@]}"; do
  [ "$loc" = "$CP_LOC" ] && continue
  echo "==> Joining worker ${loc}"
  rssh "${PUBIP[$loc]}" "WGIP='${WGIP[$loc]}' JOIN='${JOIN}' bash -s" <<'REMOTE'
set -euxo pipefail
echo "KUBELET_EXTRA_ARGS=--node-ip=${WGIP}" > /etc/default/kubelet
if [ ! -f /etc/kubernetes/kubelet.conf ]; then
  eval "${JOIN}"
fi
REMOTE
done

# ---- wait for all nodes Ready, then deploy hello-world ----
echo "==> Waiting for 3 Ready nodes"
rssh "$CP_IP" 'for _ in $(seq 1 60); do r=$(kubectl get nodes --no-headers 2>/dev/null | awk "\$2==\"Ready\"{c++} END{print c+0}"); [ "$r" -ge 3 ] && break; sleep 10; done; kubectl get nodes -o wide'
echo "==> Deploying hello-world (one pod per node)"
rssh "$CP_IP" 'kubectl apply -f -' < "$HELLO"
rssh "$CP_IP" 'kubectl rollout status deploy/hello-world --timeout=180s'
rssh "$CP_IP" 'kubectl get pods -o wide -l app=hello-world'

PASS="$(rssh "$CP_IP" "kubectl -n argocd get secret argocd-initial-admin-secret -o jsonpath='{.data.password}' | base64 -d" 2>/dev/null || true)"

echo ""
echo "============================================================"
echo "  Single cluster up across ash + hil + fsn1 (WireGuard mesh)"
echo "============================================================"
echo "  hello-world (one pod per node):"
for loc in "${LOCS[@]}"; do
  printf '    %-5s  http://%s:30080\n' "$loc" "${PUBIP[$loc]}"
done
echo ""
echo "  ArgoCD:    https://${CP_IP}:30443   (user: admin)"
echo "  ArgoCD pw: ${PASS:-<run on ash: kubectl -n argocd get secret argocd-initial-admin-secret -o jsonpath={.data.password} | base64 -d>}"
echo ""
echo "  kubectl (from ash):  ssh -i ${SSH_KEY} root@${CP_IP} kubectl get nodes -o wide"
echo "============================================================"
