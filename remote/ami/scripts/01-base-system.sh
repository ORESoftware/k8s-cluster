#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────
# 01-base-system.sh — OS updates, base packages, kernel modules,
#                     sysctl tuning for Kubernetes
# Runs as: root
# ─────────────────────────────────────────────────────────────
set -euo pipefail

echo "══════════════════════════════════════════════════════════"
echo "  01 — Base System Packages & Kernel Config"
echo "══════════════════════════════════════════════════════════"

# ── Full system update ──
dnf update -y

# ── Development tools & build essentials ──
dnf groupinstall -y "Development Tools"

dnf install -y \
  gcc gcc-c++ make cmake autoconf automake \
  openssl-devel libffi-devel bzip2-devel readline-devel \
  zlib-devel xz-devel sqlite-devel ncurses-devel \
  gettext-devel expat-devel libcurl-devel perl-ExtUtils-MakeMaker \
  tar gzip unzip wget curl \
  git \
  mercurial \
  jq \
  tmux \
  screen \
  rsync \
  which \
  bash-completion \
  man-pages \
  htop \
  tree \
  lsof \
  strace \
  perf \
  sysstat \
  procps-ng \
  util-linux \
  findutils \
  bind-utils \
  systemd-resolved

# ── Install latest Git from source if repo version is old ──
GIT_VERSION=$(git --version | awk '{print $3}')
echo "System git version: ${GIT_VERSION}"

echo "Installing latest Git release from source..."
GIT_LATEST_TAG=$(curl -fsSL https://api.github.com/repos/git/git/releases/latest | jq -r .tag_name)
GIT_LATEST_VERSION="${GIT_LATEST_TAG#v}"
mkdir -p /tmp/git-latest-src
curl -fsSL "https://github.com/git/git/archive/refs/tags/${GIT_LATEST_TAG}.tar.gz" -o /tmp/git-latest.tar.gz
tar -xzf /tmp/git-latest.tar.gz --strip-components=1 -C /tmp/git-latest-src
make -C /tmp/git-latest-src prefix=/usr/local all
make -C /tmp/git-latest-src prefix=/usr/local install
hash -r
echo "Installed git version: $(git --version) (${GIT_LATEST_VERSION})"

# ── yq YAML processor ──
echo "Installing yq..."
curl -fsSL https://github.com/mikefarah/yq/releases/latest/download/yq_linux_amd64 -o /tmp/yq
install -m 0755 /tmp/yq /usr/local/bin/yq

# ── Kernel modules required by Kubernetes / containerd ──
cat > /etc/modules-load.d/k8s.conf <<'EOF'
overlay
br_netfilter
ip_vs
ip_vs_rr
ip_vs_wrr
ip_vs_sh
nf_conntrack
EOF

modprobe overlay
modprobe br_netfilter
modprobe ip_vs
modprobe ip_vs_rr
modprobe ip_vs_wrr
modprobe ip_vs_sh
modprobe nf_conntrack || true

# ── Sysctl settings for Kubernetes networking ──
cat > /etc/sysctl.d/99-k8s.conf <<'EOF'
# Bridge traffic visible to iptables
net.bridge.bridge-nf-call-iptables  = 1
net.bridge.bridge-nf-call-ip6tables = 1

# IP forwarding (required for pod networking)
net.ipv4.ip_forward = 1

# Increase inotify limits for large clusters
fs.inotify.max_user_watches  = 524288
fs.inotify.max_user_instances = 8192

# Increase file descriptor limits
fs.file-max = 2097152

# Connection tracking table size
net.netfilter.nf_conntrack_max = 1048576

# Increase ARP cache for large clusters
net.ipv4.neigh.default.gc_thresh1 = 4096
net.ipv4.neigh.default.gc_thresh2 = 8192
net.ipv4.neigh.default.gc_thresh3 = 16384
EOF

sysctl --system

# ── Disable swap (required by kubelet) ──
swapoff -a || true
# Remove swap entries from fstab
grep -v swap /etc/fstab > /tmp/fstab.noswap && cp /tmp/fstab.noswap /etc/fstab || true

# ── Increase open file limits ──
cat > /etc/security/limits.d/99-k8s.conf <<'EOF'
*       soft    nofile    1048576
*       hard    nofile    1048576
*       soft    nproc     unlimited
*       hard    nproc     unlimited
root    soft    nofile    1048576
root    hard    nofile    1048576
EOF

echo "✅ 01 — Base system configuration complete"
