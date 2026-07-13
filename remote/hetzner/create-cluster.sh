#!/usr/bin/env bash
# =============================================================================
# create-cluster.sh — provision the Hetzner servers for the WireGuard-meshed
# single K8s cluster (control-plane in ash, workers in hil + fsn1).
#
# This step only CREATES the boxes (CCX53 / Ubuntu 24.04) with cloud-init.yaml
# as user-data (host prereqs + WireGuard keypair). Run setup-cluster.sh after
# to mesh WireGuard, kubeadm init/join, and deploy the demo.
#
# Prereqs:
#   - hcloud authenticated:  hcloud context create dd-hetzner
#   - SSH public key at $SSH_PUB (default ~/.ssh/id_hetzner.pub)
#
# Usage:
#   ./create-cluster.sh                 # ash hil fsn1 (default fleet)
#   ./create-cluster.sh ash             # one box
#
# Env overrides: SERVER_TYPE, IMAGE, SSH_KEY_NAME, SSH_PUB, FIREWALL (0 to skip)
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SERVER_TYPE="${SERVER_TYPE:-ccx53}"
IMAGE="${IMAGE:-ubuntu-24.04}"
SSH_KEY_NAME="${SSH_KEY_NAME:-dd-hetzner}"
SSH_PUB="${SSH_PUB:-$HOME/.ssh/id_hetzner.pub}"
FIREWALL="${FIREWALL:-1}"
FW_NAME="dd-k8s-fw"
USER_DATA="$SCRIPT_DIR/cloud-init.yaml"

LOCATIONS=("$@")
[ ${#LOCATIONS[@]} -eq 0 ] && LOCATIONS=(ash hil fsn1)

command -v hcloud >/dev/null || { echo "ERROR: hcloud not installed." >&2; exit 1; }
hcloud server-type describe "$SERVER_TYPE" >/dev/null 2>&1 || {
  echo "ERROR: no active hcloud context, or server type '$SERVER_TYPE' unavailable." >&2
  echo "       Run: hcloud context create dd-hetzner" >&2; exit 1; }
[ -f "$USER_DATA" ] || { echo "ERROR: $USER_DATA missing." >&2; exit 1; }

# ---- SSH key: reuse an already-uploaded key with the same fingerprint ----
# Hetzner rejects duplicate public keys, so match by fingerprint first and only
# upload if this key isn't already in the project (under any name).
[ -f "$SSH_PUB" ] || { echo "ERROR: SSH public key $SSH_PUB not found." >&2; exit 1; }
FPR="$(ssh-keygen -E md5 -lf "$SSH_PUB" | awk '{print $2}' | sed 's/^MD5://')"
EXISTING="$(hcloud ssh-key list -o noheader -o columns=name,fingerprint 2>/dev/null \
              | awk -v f="$FPR" '$2 == f { print $1; exit }')"
if [ -n "$EXISTING" ]; then
  SSH_KEY_NAME="$EXISTING"
  echo "==> Using existing Hetzner SSH key '$SSH_KEY_NAME' (fingerprint match)"
elif ! hcloud ssh-key describe "$SSH_KEY_NAME" >/dev/null 2>&1; then
  echo "==> Registering SSH key '$SSH_KEY_NAME' from $SSH_PUB"
  hcloud ssh-key create --name "$SSH_KEY_NAME" --public-key-from-file "$SSH_PUB"
fi

# ---- Firewall ----
#   udp 51820          WireGuard mesh  — open to all (packets are key-authenticated)
#   tcp 30080          hello-world     — open to all (demo URLs reachable anywhere)
#   tcp 22/6443/30443  SSH/API/ArgoCD  — restricted to this machine's public IPv4
if [ "$FIREWALL" = "1" ]; then
  if ! hcloud firewall describe "$FW_NAME" >/dev/null 2>&1; then
    echo "==> Creating firewall '$FW_NAME'"
    hcloud firewall create --name "$FW_NAME"
    hcloud firewall add-rule "$FW_NAME" --direction in --protocol udp --port 51820 \
      --source-ips 0.0.0.0/0 --source-ips ::/0 --description "WireGuard mesh"
    hcloud firewall add-rule "$FW_NAME" --direction in --protocol tcp --port 30080 \
      --source-ips 0.0.0.0/0 --source-ips ::/0 --description "hello-world demo"

    ADMIN_IP="$(curl -4 -fsS https://ifconfig.me 2>/dev/null || curl -4 -fsS https://api.ipify.org 2>/dev/null || true)"
    if [ -n "$ADMIN_IP" ]; then
      echo "==> Restricting SSH/API/ArgoCD to ${ADMIN_IP}/32"
      for port in 22 6443 30443; do
        hcloud firewall add-rule "$FW_NAME" --direction in --protocol tcp --port "$port" \
          --source-ips "${ADMIN_IP}/32" --description "admin ${ADMIN_IP}"
      done
    else
      echo "WARNING: no public IPv4 detected — opening SSH/API/ArgoCD to all." >&2
      for port in 22 6443 30443; do
        hcloud firewall add-rule "$FW_NAME" --direction in --protocol tcp --port "$port" \
          --source-ips 0.0.0.0/0 --source-ips ::/0 --description "open ${port}"
      done
    fi
  fi
fi

# ---- Create one server per location ----
for loc in "${LOCATIONS[@]}"; do
  name="dd-k8s-${loc}"
  if hcloud server describe "$name" >/dev/null 2>&1; then
    echo "==> $name already exists — skipping"
    continue
  fi
  echo "==> Creating $name ($SERVER_TYPE / $IMAGE) in $loc"
  args=( --name "$name" --type "$SERVER_TYPE" --image "$IMAGE" --location "$loc"
         --ssh-key "$SSH_KEY_NAME" --user-data-from-file "$USER_DATA"
         --label "role=dd-k8s" --label "managed-by=create-cluster" )
  [ "$FIREWALL" = "1" ] && args+=( --firewall "$FW_NAME" )
  hcloud server create "${args[@]}"
done

echo ""
echo "============================================================"
echo "  Servers created. Node prereqs (cloud-init) run ~5-10 min."
echo "  Next:  ./setup-cluster.sh    # mesh WireGuard + form cluster"
echo "============================================================"
hcloud server list --selector role=dd-k8s -o columns=name,ipv4,location,status
