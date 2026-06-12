#!/usr/bin/env bash
# =============================================================================
# setup-cluster-ha.sh — form a 3-node HA Kubernetes cluster across eu-central
# (fsn1 + nbg1 + hel1) over the native Hetzner private network. No WireGuard:
# the private network is the fast L2 fabric. All 3 nodes are stacked-etcd
# control planes (quorum survives losing any one datacenter) AND workers.
#
# A private Hetzner LB (dd-cp-lb, 10.20.0.5:6443) is the control-plane endpoint
# so the API survives any single control-plane node failure.
#
# Run after the 3 cx53 nodes (label role=dd-k8s) + dd-net + dd-cp-lb exist and
# cloud-init prereqs have finished. Idempotent.
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SSH_KEY="${SSH_KEY:-$HOME/.ssh/id_hetzner}"
POD_CIDR="${POD_CIDR:-10.244.0.0/16}"
HELLO="$SCRIPT_DIR/hello-world.yaml"
CRISOCK="unix:///run/containerd/containerd.sock"

LOCS=(fsn1 nbg1 hel1)     # control-plane node FIRST
CP_LOC="fsn1"

SSH_OPTS=(-i "$SSH_KEY" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 -o ServerAliveInterval=15)
rssh() { local ip="$1"; shift; ssh "${SSH_OPTS[@]}" root@"$ip" "$@"; }

command -v hcloud >/dev/null || { echo "ERROR: hcloud not authenticated." >&2; exit 1; }

# ---- resolve public + private IPs and the LB endpoint ----
declare -A PUB PRIV
for loc in "${LOCS[@]}"; do
  PUB[$loc]="$(hcloud server ip "dd-k8s-${loc}")"
  PRIV[$loc]="$(hcloud server describe "dd-k8s-${loc}" -o json | jq -r '.private_net[0].ip')"
  [ -n "${PRIV[$loc]}" ] || { echo "ERROR: no private IP for dd-k8s-${loc}" >&2; exit 1; }
  ssh-keygen -R "${PUB[$loc]}" >/dev/null 2>&1 || true   # Hetzner reuses IPs; drop stale host keys
done
LB_IP="$(hcloud load-balancer describe dd-cp-lb -o json | jq -r '.private_net[0].ip')"
CPE="${LB_IP}:6443"
echo "  control-plane endpoint (LB): ${CPE}"
for loc in "${LOCS[@]}"; do echo "  dd-k8s-${loc}: pub ${PUB[$loc]}  priv ${PRIV[$loc]}"; done

# ---- wait for cloud-init prereqs ----
for loc in "${LOCS[@]}"; do
  echo "==> waiting for prereqs on ${loc} (${PUB[$loc]})"
  for _ in $(seq 1 90); do
    rssh "${PUB[$loc]}" 'test -f /opt/dd/prereqs-complete' 2>/dev/null && break
    sleep 10
  done
  rssh "${PUB[$loc]}" 'test -f /opt/dd/prereqs-complete' \
    || { echo "ERROR: prereqs never finished on ${loc}" >&2; exit 1; }
done

# ---- control-plane #1: kubeadm init (HA endpoint) + Cilium + storage + ArgoCD ----
CP1="${PUB[$CP_LOC]}"
echo "==> kubeadm init on ${CP_LOC}"
rssh "$CP1" "CPE='${CPE}' ADV='${PRIV[$CP_LOC]}' LB='${LB_IP}' POD_CIDR='${POD_CIDR}' CRISOCK='${CRISOCK}' bash -s" <<'REMOTE'
set -euxo pipefail
echo "KUBELET_EXTRA_ARGS=--node-ip=${ADV}" > /etc/default/kubelet
if [ ! -f /etc/kubernetes/admin.conf ]; then
  kubeadm init \
    --control-plane-endpoint="${CPE}" \
    --upload-certs \
    --apiserver-advertise-address="${ADV}" \
    --apiserver-cert-extra-sans="${LB}" \
    --pod-network-cidr="${POD_CIDR}" \
    --skip-phases=addon/kube-proxy \
    --cri-socket="${CRISOCK}" | tee /var/log/kubeadm-init.log
fi
mkdir -p /root/.kube && cp -f /etc/kubernetes/admin.conf /root/.kube/config
export KUBECONFIG=/etc/kubernetes/admin.conf

# Cilium points at the LB endpoint so pod networking survives a CP node loss
if ! cilium status >/dev/null 2>&1; then
  cilium install --set kubeProxyReplacement=true --set ipam.mode=kubernetes \
    --set k8sServiceHost="${LB}" --set k8sServicePort=6443
fi
cilium status --wait --wait-duration 5m || echo "(cilium settling)"

# default StorageClass gp3 (local-path)
LP_TAG="$(curl -fsSL https://api.github.com/repos/rancher/local-path-provisioner/releases/latest | jq -r .tag_name)"
kubectl apply -f "https://raw.githubusercontent.com/rancher/local-path-provisioner/${LP_TAG}/deploy/local-path-storage.yaml"
kubectl apply -f - <<'SC'
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: gp3
  annotations: { storageclass.kubernetes.io/is-default-class: "true" }
provisioner: rancher.io/local-path
volumeBindingMode: WaitForFirstConsumer
reclaimPolicy: Delete
SC

# ArgoCD (server-side apply: CRDs exceed client-side annotation limit)
ARGOCD_TAG="$(curl -fsSL https://api.github.com/repos/argoproj/argo-cd/releases/latest | jq -r .tag_name)"
kubectl create namespace argocd 2>/dev/null || true
kubectl apply --server-side --force-conflicts -n argocd -f "https://raw.githubusercontent.com/argoproj/argo-cd/${ARGOCD_TAG}/manifests/install.yaml"
kubectl -n argocd rollout status deploy/argocd-server --timeout=300s || true
kubectl -n argocd patch svc argocd-server -p '{"spec":{"type":"NodePort","ports":[{"name":"https","port":443,"targetPort":8080,"nodePort":30443}]}}' || true
REMOTE

# ---- wait for the LB to route to CP1's healthy apiserver before joining ----
echo "==> waiting for LB endpoint ${CPE} to become reachable"
for _ in $(seq 1 30); do
  rssh "$CP1" "timeout 3 bash -c '</dev/tcp/${LB_IP}/6443'" 2>/dev/null && break
  sleep 5
done

# ---- join the other two as control-plane nodes (stacked etcd) ----
CERT_KEY="$(rssh "$CP1" 'kubeadm init phase upload-certs --upload-certs 2>/dev/null | tail -1 | tr -d "[:space:]"')"
JOIN="$(rssh "$CP1" 'kubeadm token create --print-join-command')"
for loc in "${LOCS[@]}"; do
  [ "$loc" = "$CP_LOC" ] && continue
  echo "==> joining control-plane ${loc}"
  rssh "${PUB[$loc]}" "ADV='${PRIV[$loc]}' JOIN='${JOIN}' CK='${CERT_KEY}' CRISOCK='${CRISOCK}' bash -s" <<'REMOTE'
set -euxo pipefail
echo "KUBELET_EXTRA_ARGS=--node-ip=${ADV}" > /etc/default/kubelet
if [ ! -f /etc/kubernetes/kubelet.conf ]; then
  eval "${JOIN} --control-plane --certificate-key ${CK} --apiserver-advertise-address=${ADV} --cri-socket=${CRISOCK}"
fi
REMOTE
done

# ---- untaint (all 3 run workloads), wait Ready, deploy hello-world ----
rssh "$CP1" 'export KUBECONFIG=/etc/kubernetes/admin.conf; kubectl taint nodes --all node-role.kubernetes.io/control-plane- 2>/dev/null || true'
echo "==> waiting for 3 Ready nodes"
rssh "$CP1" 'export KUBECONFIG=/etc/kubernetes/admin.conf; for _ in $(seq 1 60); do r=$(kubectl get nodes --no-headers 2>/dev/null | awk "\$2==\"Ready\"{c++} END{print c+0}"); [ "$r" -ge 3 ] && break; sleep 10; done; kubectl get nodes -o wide'
echo "==> deploying hello-world"
rssh "$CP1" 'kubectl apply -f -' < "$HELLO"
rssh "$CP1" 'export KUBECONFIG=/etc/kubernetes/admin.conf; kubectl rollout status deploy/hello-world --timeout=180s; kubectl get pods -o wide -l app=hello-world'

PASS="$(rssh "$CP1" "kubectl -n argocd get secret argocd-initial-admin-secret -o jsonpath='{.data.password}' | base64 -d" 2>/dev/null || true)"
echo ""
echo "============================================================"
echo "  HA cluster up: 3 stacked-etcd control planes (eu-central)"
echo "============================================================"
echo "  etcd quorum survives losing any one of fsn1 / nbg1 / hel1."
echo "  control-plane endpoint (private LB): ${CPE}"
for loc in "${LOCS[@]}"; do printf "    %-5s pub %s\n" "$loc" "${PUB[$loc]}"; done
echo "  ArgoCD:    https://${PUB[$CP_LOC]}:30443   (user: admin)"
echo "  ArgoCD pw: ${PASS:-fetch on a node from secret argocd-initial-admin-secret}"
echo "============================================================"
