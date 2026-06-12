#!/usr/bin/env bash
# =============================================================================
# storage-csi-ha.sh — make `gp3` durable network block storage (Hetzner CSI),
# so storage semantics match AWS EBS and workloads are portable AWS<->Hetzner.
#
# Steps:
#   1. set each node's providerID = hcloud://<server-id>  (CSI needs this; we
#      run bare kubeadm without the cloud-controller-manager, so set it by hand)
#   2. create the in-cluster `hcloud` token secret (from $HCLOUD_TOKEN or the
#      active hcloud context)
#   3. install the hcloud-csi driver (helm)
#   4. repoint the default StorageClass `gp3` from local-path to csi.hetzner.cloud
#
# Idempotent. Run after setup-cluster-ha.sh. No stateful PVCs are migrated;
# existing local-path PVs (if any) keep working under their own StorageClass.
# =============================================================================
set -euo pipefail

SSH_KEY="${SSH_KEY:-$HOME/.ssh/id_hetzner}"
SSH_OPTS=(-i "$SSH_KEY" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15)
rssh() { local ip="$1"; shift; ssh "${SSH_OPTS[@]}" root@"$ip" "$@"; }

command -v hcloud >/dev/null || { echo "ERROR: hcloud not authenticated." >&2; exit 1; }
TOKEN="${HCLOUD_TOKEN:-$(awk -F'"' '/token *=/{print $2; exit}' "$HOME/.config/hcloud/cli.toml" 2>/dev/null)}"
[ -n "$TOKEN" ] || { echo "ERROR: no hcloud token (set HCLOUD_TOKEN)." >&2; exit 1; }

mapfile -t NODES < <(hcloud server list --selector role=dd-k8s -o noheader -o columns=name | sort)
CP1_IP="$(hcloud server ip "${NODES[0]}")"; ssh-keygen -R "$CP1_IP" >/dev/null 2>&1 || true

# 1. providerIDs (immutable once set)
echo "==> setting node providerIDs"
hcloud server list --selector role=dd-k8s -o json | jq -r '.[] | "\(.name) \(.id)"' | while read -r name id; do
  rssh "$CP1_IP" "export KUBECONFIG=/etc/kubernetes/admin.conf
    cur=\$(kubectl get node $name -o jsonpath='{.spec.providerID}')
    [ -z \"\$cur\" ] && kubectl patch node $name --type merge -p '{\"spec\":{\"providerID\":\"hcloud://$id\"}}' >/dev/null && echo '  set hcloud://$id on $name' || echo \"  $name: \${cur:-unchanged}\""
done

# 2-4. token secret + driver + gp3 SC (on the control plane)
echo "==> installing hcloud-csi + repointing gp3"
rssh "$CP1_IP" "TOKEN='${TOKEN}' bash -s" <<'REMOTE'
set -e
export KUBECONFIG=/etc/kubernetes/admin.conf
kubectl -n kube-system create secret generic hcloud --from-literal=token="${TOKEN}" --dry-run=client -o yaml | kubectl apply -f - >/dev/null
helm repo add hcloud https://charts.hetzner.cloud >/dev/null 2>&1 || true
helm repo update >/dev/null 2>&1
helm upgrade --install hcloud-csi hcloud/hcloud-csi -n kube-system >/dev/null
kubectl -n kube-system rollout status deploy/hcloud-csi-controller --timeout=180s
kubectl delete storageclass gp3 --ignore-not-found >/dev/null
for sc in $(kubectl get sc -o name 2>/dev/null); do kubectl annotate "$sc" storageclass.kubernetes.io/is-default-class- >/dev/null 2>&1 || true; done
kubectl apply -f - <<SC
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: gp3
  annotations:
    storageclass.kubernetes.io/is-default-class: "true"
provisioner: csi.hetzner.cloud
volumeBindingMode: WaitForFirstConsumer
allowVolumeExpansion: true
reclaimPolicy: Delete
SC
kubectl get sc
REMOTE
echo "==> gp3 is now Hetzner network block storage (EBS-equivalent)."
