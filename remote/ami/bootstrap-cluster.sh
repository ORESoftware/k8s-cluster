#!/usr/bin/env bash
# =============================================================================
# bootstrap-cluster.sh — Bring up a single-node K8s cluster on this EC2.
#
# Run this ONCE after launching from the dd-k8s-node AMI. By the time it
# finishes, you have a working cluster with everything Vercel needs to
# dispatch per-thread containers — and it prints the four env vars to
# paste into the Vercel project.
#
# Single-node by design — scale the EC2 box up rather than adding more
# nodes; see remote/k8s/readme.md for sizing.
#
# Usage (from a repo checkout):
#   sudo ./remote/ami/bootstrap-cluster.sh [--pod-cidr CIDR] [--kubelet-max-pods N] [--manifests-dir DIR]
#
# Defaults:
#   --pod-cidr        10.244.0.0/16
#   --kubelet-max-pods 220
#   --manifests-dir    $(dirname "$0")/../k8s     (i.e. remote/k8s/)
#
# Before running, fill in real secret values:
#   cp $MANIFESTS_DIR/02-secrets.template.yaml $MANIFESTS_DIR/02-secrets.yaml
#   $EDITOR $MANIFESTS_DIR/02-secrets.yaml
#
# What the script does, in order:
#   1.  start containerd, disable swap
#   2.  kubeadm init (kube-proxy skipped — Cilium replaces it)
#   3.  configure kubectl for the invoking user (works under sudo)
#   4.  untaint the control plane so workloads schedule on it
#   5.  install Cilium CNI (eBPF — networking + NetworkPolicy enforcement)
#   6.  install ArgoCD on NodePort 30443 (browser cluster visualization)
#   7.  install ingress-nginx, cert-manager, AWS EBS CSI driver, gp3 StorageClass
#   8.  apply the dd-dev manifests (00, 01, 02, 03, 04, 05)
#   9.  extract the dd-control-plane SA token and print Vercel env vars
# =============================================================================
set -euo pipefail

POD_CIDR="10.244.0.0/16"
KUBELET_MAX_PODS="${KUBELET_MAX_PODS:-220}"
DD_NAMESPACE="dd-dev"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MANIFESTS_DIR="${MANIFESTS_DIR:-$SCRIPT_DIR/../k8s}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --pod-cidr) POD_CIDR="$2"; shift 2 ;;
    --kubelet-max-pods) KUBELET_MAX_PODS="$2"; shift 2 ;;
    --manifests-dir) MANIFESTS_DIR="$2"; shift 2 ;;
    -h|--help)
      awk 'NR >= 2 && NR <= 33 { sub(/^# ?/, ""); print }' "$0"
      exit 0
      ;;
    *) echo "Unknown arg: $1" >&2; exit 1 ;;
  esac
done

if [[ ! -f "$MANIFESTS_DIR/00-namespace.yaml" ]]; then
  echo "ERROR: dd-dev manifests not found at $MANIFESTS_DIR" >&2
  echo "       Run from the dd-next-1 checkout, or pass --manifests-dir DIR." >&2
  exit 1
fi
if [[ ! -f "$MANIFESTS_DIR/02-secrets.yaml" ]]; then
  echo "ERROR: $MANIFESTS_DIR/02-secrets.yaml is missing." >&2
  echo "       Copy 02-secrets.template.yaml to 02-secrets.yaml and fill in" >&2
  echo "       real values before bootstrapping." >&2
  exit 1
fi

# Whoever invoked us via sudo — we want kubectl wired up for *them*, not for root.
TARGET_USER="${SUDO_USER:-$(whoami)}"
TARGET_HOME="$(getent passwd "$TARGET_USER" | cut -d: -f6)"
NODE_IP="$(hostname -I | awk '{print $1}')"

configure_kubelet_max_pods() {
  local config="/var/lib/kubelet/config.yaml"

  if [[ ! "${KUBELET_MAX_PODS}" =~ ^[0-9]+$ ]]; then
    echo "ERROR: --kubelet-max-pods must be a positive integer." >&2
    exit 1
  fi

  if (( KUBELET_MAX_PODS < 110 || KUBELET_MAX_PODS > 240 )); then
    cat >&2 <<EOF
ERROR: --kubelet-max-pods=${KUBELET_MAX_PODS} is outside the supported range.

This single-node Cilium cluster gets a /24 pod CIDR by default, so keep the
kubelet ceiling between 110 and 240 pods unless the pod CIDR sizing changes too.
EOF
    exit 1
  fi

  if [[ ! -f "${config}" ]]; then
    echo "ERROR: kubelet config not found at ${config}" >&2
    exit 1
  fi

  echo "==> Setting kubelet maxPods=${KUBELET_MAX_PODS}"
  awk -v max_pods="${KUBELET_MAX_PODS}" '
    BEGIN { wrote = 0 }
    /^maxPods:/ { print "maxPods: " max_pods; wrote = 1; next }
    { print }
    END { if (wrote == 0) print "maxPods: " max_pods }
  ' "${config}" > /tmp/dd-kubelet-config.yaml
  cp /tmp/dd-kubelet-config.yaml "${config}"

  # Reserve headroom for the OS, kubelet, container runtime, and control plane
  # so workloads can never starve them. Without this, allocatable == capacity and
  # a workload burst (e.g. a cold-start build stampede) can wedge kubelet/apiserver.
  #
  # Sized proportionally to the box (single-node cluster you scale up, not out):
  #   - per-pool CPU  ~6% of cores, clamped to [250m, 1000m]
  #   - per-pool mem  ~5% of RAM,   clamped to [512Mi, 2048Mi]
  #   - hard mem-eviction floor ~2% of RAM, clamped to [512Mi, 2048Mi]
  # On the target x8i.2xlarge (8 vCPU / 128 GiB) this reserves ~1 vCPU + ~4 GiB
  # total, leaving allocatable ~7 vCPU so kube-system pods still fit alongside
  # the dd-dev ResourceQuota (requests.cpu=6). A flat 1000m/pool reserved 2 of 8
  # vCPU (25%) and collided with that quota; flat values also over-reserve on the
  # 4-vCPU smoke box.
  if ! grep -q '^systemReserved:' "${config}"; then
    local total_mcpu total_mem_mi res_cpu res_mem evict_mem
    total_mcpu=$(( $(nproc) * 1000 ))
    total_mem_mi=$(( $(awk '/^MemTotal:/ {print $2}' /proc/meminfo) / 1024 ))

    res_cpu=$(( total_mcpu * 6 / 100 ))
    if (( res_cpu < 250 ));  then res_cpu=250;  fi
    if (( res_cpu > 1000 )); then res_cpu=1000; fi

    res_mem=$(( total_mem_mi * 5 / 100 ))
    if (( res_mem < 512 ));  then res_mem=512;  fi
    if (( res_mem > 2048 )); then res_mem=2048; fi

    evict_mem=$(( total_mem_mi * 2 / 100 ))
    if (( evict_mem < 512 ));  then evict_mem=512;  fi
    if (( evict_mem > 2048 )); then evict_mem=2048; fi

    echo "==> Adding kubelet reservations: system+kube ${res_cpu}m cpu / ${res_mem}Mi each (evict <${evict_mem}Mi avail)"
    cat >> "${config}" <<KUBELET_RESERVATIONS
systemReserved:
  cpu: ${res_cpu}m
  memory: ${res_mem}Mi
kubeReserved:
  cpu: ${res_cpu}m
  memory: ${res_mem}Mi
evictionHard:
  memory.available: ${evict_mem}Mi
  nodefs.available: 10%
  imagefs.available: 10%
KUBELET_RESERVATIONS
  fi

  systemctl restart kubelet
}

echo "============================================"
echo "  DD K8s Cluster Bootstrap (single-node)"
echo "============================================"
echo "  Node IP:    $NODE_IP"
echo "  Pod CIDR:   $POD_CIDR"
echo "  Max pods:   $KUBELET_MAX_PODS"
echo "  Namespace:  $DD_NAMESPACE"
echo "  Manifests:  $MANIFESTS_DIR"
echo "  kubectl-as: $TARGET_USER"
echo ""

# ---- 1. containerd + swapoff ----
echo "==> Starting containerd"
systemctl enable --now containerd
echo "==> Disabling swap (runtime + /etc/fstab so it survives reboot)"
swapoff -a || true
# Comment out any active swap line in fstab so kubelet starts after reboot.
cp /etc/fstab /etc/fstab.bak || true
awk '
  /^[^#].*[[:space:]]swap[[:space:]]/ { print "# " $0; next }
  { print }
' /etc/fstab > /tmp/dd-fstab-noswap && cp /tmp/dd-fstab-noswap /etc/fstab || true

# ---- 2. kubeadm init ----
echo "==> Initializing control plane with kubeadm"
kubeadm init \
  --pod-network-cidr="$POD_CIDR" \
  --skip-phases=addon/kube-proxy \
  --cri-socket=unix:///run/containerd/containerd.sock \
  2>&1 | tee /tmp/kubeadm-init.log
configure_kubelet_max_pods

# ---- 3. Configure kubectl for the invoking user ----
echo "==> Configuring kubectl for $TARGET_USER"
mkdir -p "$TARGET_HOME/.kube"
cp -f /etc/kubernetes/admin.conf "$TARGET_HOME/.kube/config"
chown -R "$TARGET_USER:$TARGET_USER" "$TARGET_HOME/.kube"
export KUBECONFIG="$TARGET_HOME/.kube/config"

# ---- 4. Untaint single-node control plane ----
echo "==> Untainting control plane for workload scheduling"
kubectl taint nodes --all node-role.kubernetes.io/control-plane- 2>/dev/null || true

# ---- 5. Cilium CNI ----
# Cilium replaces kube-proxy AND enforces NetworkPolicy via eBPF.
# 04-network-policy.yaml is only meaningful because of this.
echo "==> Installing Cilium CNI"
# ipam.mode=kubernetes: Cilium honors the kubeadm-allocated pod CIDR
# (10.244.0.0/16) instead of falling back to its own cluster-pool default.
cilium install \
  --set kubeProxyReplacement=true \
  --set ipam.mode=kubernetes \
  --set k8sServiceHost="$NODE_IP" \
  --set k8sServicePort=6443
cilium status --wait --wait-duration 5m || echo "(Cilium still initializing — keep going.)"

# ---- 6. ArgoCD ----
echo "==> Installing ArgoCD (NodePort 30443)"
kubectl create namespace argocd 2>/dev/null || true
kubectl apply -n argocd -f /opt/dd/manifests/argocd-install.yaml
kubectl patch svc argocd-server -n argocd \
  -p '{"spec": {"type": "NodePort", "ports": [{"port": 443, "targetPort": 8080, "nodePort": 30443}]}}' \
  || true

# ---- 7. Cluster add-ons (ingress-nginx, cert-manager, EBS CSI, gp3 SC) ----
echo "==> Adding Helm repos"
helm repo add ingress-nginx https://kubernetes.github.io/ingress-nginx >/dev/null 2>&1 || true
helm repo add aws-ebs-csi-driver https://kubernetes-sigs.github.io/aws-ebs-csi-driver >/dev/null 2>&1 || true
helm repo update >/dev/null

echo "==> Installing ingress-nginx"
helm upgrade --install ingress-nginx ingress-nginx/ingress-nginx \
  --namespace ingress-nginx --create-namespace

echo "==> Installing cert-manager"
kubectl apply -f https://github.com/cert-manager/cert-manager/releases/latest/download/cert-manager.yaml

echo "==> Installing AWS EBS CSI driver"
helm upgrade --install aws-ebs-csi-driver aws-ebs-csi-driver/aws-ebs-csi-driver \
  --namespace kube-system

echo "==> Applying gp3 default StorageClass"
kubectl apply -f - <<'EOF'
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: gp3
  annotations: { storageclass.kubernetes.io/is-default-class: "true" }
provisioner: ebs.csi.aws.com
volumeBindingMode: WaitForFirstConsumer
parameters:
  type: gp3
  fsType: ext4
EOF

# ---- 8. dd-dev manifests ----
echo "==> Applying dd-dev manifests from $MANIFESTS_DIR"
kubectl apply -f "$MANIFESTS_DIR/00-namespace.yaml"
kubectl apply -f "$MANIFESTS_DIR/01-configmap.yaml"
kubectl apply -f "$MANIFESTS_DIR/02-secrets.yaml"
kubectl apply -f "$MANIFESTS_DIR/03-rbac.yaml"
kubectl apply -f "$MANIFESTS_DIR/04-network-policy.yaml"
kubectl apply -f "$MANIFESTS_DIR/05-resource-quota.yaml"
# 06-/07-/08-/09- are per-thread templates instantiated by the orchestrator on first dispatch.

# ---- 9. Extract dd-control-plane SA token ----
echo "==> Waiting for dd-control-plane SA token"
for _ in {1..30}; do
  if kubectl -n "$DD_NAMESPACE" get secret dd-control-plane-token \
      -o jsonpath='{.data.token}' >/dev/null 2>&1; then
    break
  fi
  sleep 2
done
SA_TOKEN="$(kubectl -n "$DD_NAMESPACE" get secret dd-control-plane-token \
            -o jsonpath='{.data.token}' | base64 -d)"
K8S_API="https://${NODE_IP}:6443"

echo ""
echo "============================================"
echo "  Cluster Bootstrap Complete!"
echo "============================================"
echo ""
echo "  K8s API:      $K8S_API"
echo "  ArgoCD UI:    https://${NODE_IP}:30443"
echo "  Namespace:    $DD_NAMESPACE"
echo ""
echo "  --- Vercel env vars (paste into the project) ---"
echo ""
echo "  K8S_API_SERVER=$K8S_API"
echo "  K8S_NAMESPACE=$DD_NAMESPACE"
echo "  K8S_SA_TOKEN=$SA_TOKEN"
echo "  K8S_INSECURE_TLS=true"
echo ""
echo "  --- ArgoCD admin password ---"
echo "  kubectl -n argocd get secret argocd-initial-admin-secret \\"
echo "    -o jsonpath='{.data.password}' | base64 -d; echo"
echo ""
echo "  --- Quick-look commands ---"
echo "  kubectl get nodes"
echo "  kubectl get pods -A"
echo "  cilium status"
echo "  k9s -n $DD_NAMESPACE"
echo "  stern -n $DD_NAMESPACE ."
echo ""
echo "============================================"
