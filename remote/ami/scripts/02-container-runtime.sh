#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────
# 02-container-runtime.sh — containerd, crictl, nerdctl, runc, CNI plugins
# Runs as: root
# Env:
#   K8S_VERSION  — Kubernetes minor version (e.g. "1.31")
# ─────────────────────────────────────────────────────────────
set -euo pipefail

echo "══════════════════════════════════════════════════════════"
echo "  02 — Container Runtime (containerd + tooling)"
echo "══════════════════════════════════════════════════════════"

ARCH="amd64"
CONTAINERD_VERSION="1.7.24"
RUNC_VERSION="1.2.3"
CNI_VERSION="1.6.1"
CRICTL_VERSION="1.31.1"
NERDCTL_VERSION="2.0.2"

# ── containerd ──
echo "→ Installing containerd ${CONTAINERD_VERSION}..."
curl -fsSL "https://github.com/containerd/containerd/releases/download/v${CONTAINERD_VERSION}/containerd-${CONTAINERD_VERSION}-linux-${ARCH}.tar.gz" \
  | tar -xzC /usr/local

# systemd unit for containerd
mkdir -p /usr/local/lib/systemd/system
curl -fsSL "https://raw.githubusercontent.com/containerd/containerd/v${CONTAINERD_VERSION}/containerd.service" \
  -o /usr/local/lib/systemd/system/containerd.service

# Configure containerd with SystemdCgroup
mkdir -p /etc/containerd
containerd config default > /etc/containerd/config.toml

# Enable SystemdCgroup (required for kubeadm)
if grep -q 'SystemdCgroup' /etc/containerd/config.toml; then
  # Use awk for a targeted config rewrite.
  awk '{gsub(/SystemdCgroup = false/, "SystemdCgroup = true")}1' \
    /etc/containerd/config.toml > /tmp/containerd-config.toml \
    && cp /tmp/containerd-config.toml /etc/containerd/config.toml
fi

systemctl daemon-reload
systemctl enable containerd
systemctl start containerd

# ── runc ──
echo "→ Installing runc ${RUNC_VERSION}..."
curl -fsSL "https://github.com/opencontainers/runc/releases/download/v${RUNC_VERSION}/runc.${ARCH}" \
  -o /usr/local/sbin/runc
chmod 755 /usr/local/sbin/runc

# ── CNI plugins ──
echo "→ Installing CNI plugins ${CNI_VERSION}..."
mkdir -p /opt/cni/bin
curl -fsSL "https://github.com/containernetworking/plugins/releases/download/v${CNI_VERSION}/cni-plugins-linux-${ARCH}-v${CNI_VERSION}.tgz" \
  | tar -xzC /opt/cni/bin

# ── crictl ──
echo "→ Installing crictl ${CRICTL_VERSION}..."
curl -fsSL "https://github.com/kubernetes-sigs/cri-tools/releases/download/v${CRICTL_VERSION}/crictl-v${CRICTL_VERSION}-linux-${ARCH}.tar.gz" \
  | tar -xzC /usr/local/bin

cat > /etc/crictl.yaml <<'EOF'
runtime-endpoint: unix:///run/containerd/containerd.sock
image-endpoint: unix:///run/containerd/containerd.sock
timeout: 10
debug: false
EOF

# ── nerdctl (Docker-compatible CLI for containerd) ──
echo "→ Installing nerdctl ${NERDCTL_VERSION}..."
curl -fsSL "https://github.com/containerd/nerdctl/releases/download/v${NERDCTL_VERSION}/nerdctl-${NERDCTL_VERSION}-linux-${ARCH}.tar.gz" \
  | tar -xzC /usr/local/bin

# ── ctr is already included with containerd ──
echo "→ ctr (containerd CLI) available at $(which ctr)"

# ── Verify installations ──
echo ""
echo "─── Container Runtime Versions ───"
containerd --version
runc --version
crictl --version
nerdctl --version
ctr --version

echo "✅ 02 — Container runtime installation complete"
