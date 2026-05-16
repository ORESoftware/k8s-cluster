#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# 04-networking.sh - networking diagnostics, Cilium/Hubble, and eBPF tools
# Runs as: root
# ---------------------------------------------------------------------------
set -euo pipefail

echo "=========================================================="
echo "  04 - Networking, CNI, and eBPF Tooling"
echo "=========================================================="

ARCH="amd64"

install_optional_packages() {
  for package_name in "$@"; do
    echo "Installing package: ${package_name}"
    dnf install -y "${package_name}" || echo "Warning: ${package_name} was not available from enabled repos"
  done
}

install_optional_packages \
  iproute \
  iptables \
  iptables-nft \
  iputils \
  tcpdump \
  traceroute \
  nmap \
  nmap-ncat \
  socat \
  mtr \
  conntrack-tools \
  ethtool \
  bridge-utils \
  bind-utils \
  bpftool \
  bpftrace \
  bcc \
  bcc-tools \
  kernel-devel \
  kernel-headers

echo "Installing Cilium CLI..."
CILIUM_TAG=$(curl -fsSL https://api.github.com/repos/cilium/cilium-cli/releases/latest | jq -r .tag_name)
curl -fsSL "https://github.com/cilium/cilium-cli/releases/download/${CILIUM_TAG}/cilium-linux-${ARCH}.tar.gz" -o /tmp/cilium.tgz
tar -xzf /tmp/cilium.tgz -C /tmp
install -m 0755 /tmp/cilium /usr/local/bin/cilium

echo "Installing Hubble CLI..."
HUBBLE_TAG=$(curl -fsSL https://api.github.com/repos/cilium/hubble/releases/latest | jq -r .tag_name)
curl -fsSL "https://github.com/cilium/hubble/releases/download/${HUBBLE_TAG}/hubble-linux-${ARCH}.tar.gz" -o /tmp/hubble.tgz
tar -xzf /tmp/hubble.tgz -C /tmp
install -m 0755 /tmp/hubble /usr/local/bin/hubble

echo ""
echo "--- Networking and eBPF Tool Versions ---"
ip -V
iptables --version || true
tcpdump --version | head -1 || true
traceroute --version 2>&1 | head -1 || true
nmap --version | head -1 || true
nc -h 2>&1 | head -1 || true
socat -V | head -1 || true
mtr --version || true
conntrack -V || true
bpftool version || true
bpftrace --version || true
cilium version --client || true
hubble version || true

echo "04 - Networking, CNI, and eBPF tooling installation complete"
