#!/usr/bin/env bash
# Bootstrap a stock Amazon Linux 2023 EC2 instance for the dd remote-dev
# kubeadm cluster. This is the "no custom AMI yet" path; the AMI remains
# the faster production path once it is available.
set -euo pipefail

K8S_VERSION="${K8S_VERSION:-1.31}"
CONTAINERD_VERSION="${CONTAINERD_VERSION:-1.7.24}"
RUNC_VERSION="${RUNC_VERSION:-1.2.3}"
CNI_VERSION="${CNI_VERSION:-1.9.1}"
CRICTL_VERSION="${CRICTL_VERSION:-1.31.1}"
NERDCTL_VERSION="${NERDCTL_VERSION:-2.0.2}"
POD_CIDR="${POD_CIDR:-10.244.0.0/16}"
DD_ALLOW_UNDERSIZED="${DD_ALLOW_UNDERSIZED:-0}"
DD_SKIP_DNF_UPDATE="${DD_SKIP_DNF_UPDATE:-0}"

MODE="all"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
MANIFESTS_DIR="${MANIFESTS_DIR:-${REPO_ROOT}/remote/k8s}"

usage() {
  cat <<'USAGE'
Usage:
  remote/ec2/bootstrap-amazon-linux-2023-k8s.sh [mode] [options]

Modes:
  --all             Install prerequisites and run kubeadm cluster bootstrap.
  --prereqs-only    Install OS, containerd, Kubernetes, Helm, Cilium tooling.
  --cluster-only    Run the cluster bootstrap using already-installed tools.

Options:
  --force-small-instance
                   Allow the cluster phase on nodes below kubeadm's practical
                   memory floor. Useful only for throwaway smoke tests.
  --manifests-dir DIR
                   Directory containing remote/k8s manifests.
  --pod-cidr CIDR  Pod CIDR passed through to kubeadm bootstrap.
  -h, --help       Show this help.

Environment:
  K8S_VERSION=1.31
  DD_ALLOW_UNDERSIZED=1
  DD_SKIP_DNF_UPDATE=1

Before --all or --cluster-only, create remote/k8s/02-secrets.yaml with real
values. The script refuses placeholder secrets so a half-configured cluster
does not get exposed.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --all)
      MODE="all"
      shift
      ;;
    --prereqs-only)
      MODE="prereqs"
      shift
      ;;
    --cluster-only)
      MODE="cluster"
      shift
      ;;
    --force-small-instance)
      DD_ALLOW_UNDERSIZED=1
      shift
      ;;
    --manifests-dir)
      MANIFESTS_DIR="$2"
      shift 2
      ;;
    --pod-cidr)
      POD_CIDR="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ "${EUID}" -eq 0 ]]; then
  SUDO=()
else
  if ! command -v sudo >/dev/null 2>&1; then
    echo "ERROR: run as root or install sudo for non-root bootstrap." >&2
    exit 1
  fi
  SUDO=(sudo)
fi

run_root() {
  "${SUDO[@]}" "$@"
}

ensure_git_available() {
  if command -v git >/dev/null 2>&1; then
    return 0
  fi

  echo "==> Installing git for the initial repository sync"
  run_root dnf install -y git
}

detect_arch() {
  case "$(uname -m)" in
    x86_64) echo "amd64" ;;
    aarch64|arm64) echo "arm64" ;;
    *)
      echo "ERROR: unsupported architecture: $(uname -m)" >&2
      exit 1
      ;;
  esac
}

ARCH="$(detect_arch)"

sync_repo_to_origin_dev() {
  echo "==> Syncing repository checkout to origin/dev"
  if ! git -C "${REPO_ROOT}" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "ERROR: ${REPO_ROOT} is not a git checkout" >&2
    exit 1
  fi

  git -C "${REPO_ROOT}" fetch --prune origin dev

  if git -C "${REPO_ROOT}" rev-parse --verify dev >/dev/null 2>&1; then
    git -C "${REPO_ROOT}" switch dev
  else
    git -C "${REPO_ROOT}" switch -c dev --track origin/dev
  fi

  git -C "${REPO_ROOT}" pull --ff-only origin dev

  # Submodules — chat.vibe lives at remote/deployments/gcs/chat-vibe as one. The
  # gcs and gcs-router deployments hostPath-mount that path, so an
  # uninitialized submodule on EC2 means an empty directory and the
  # build-on-startup pods will fail. Idempotent; cheap to re-run.
  if [[ -f "${REPO_ROOT}/.gitmodules" ]]; then
    echo "==> Updating submodules"
    git -C "${REPO_ROOT}" submodule sync --recursive
    git -C "${REPO_ROOT}" submodule update --init --recursive --remote
  fi
}

capacity_summary() {
  local cpus mem_mib
  cpus="$(awk '/^processor[[:space:]]*:/{count++} END{print count + 0}' /proc/cpuinfo)"
  mem_mib="$(awk '/^MemTotal:/{print int($2 / 1024)}' /proc/meminfo)"
  echo "Detected capacity: ${cpus} vCPU, ${mem_mib} MiB memory"
}

check_capacity_for_cluster() {
  local cpus mem_mib
  cpus="$(awk '/^processor[[:space:]]*:/{count++} END{print count + 0}' /proc/cpuinfo)"
  mem_mib="$(awk '/^MemTotal:/{print int($2 / 1024)}' /proc/meminfo)"

  capacity_summary

  if (( cpus < 2 || mem_mib < 1800 )); then
    cat >&2 <<EOF
ERROR: this node is below the practical kubeadm floor.

Need at least 2 vCPU and about 2 GiB RAM for the control plane to survive
bootstrap. The repo's documented remote-dev workload target is larger:
x8i.2xlarge or a comparable 128 GiB x86 host for the long-lived node.

Set DD_ALLOW_UNDERSIZED=1 or pass --force-small-instance only for a throwaway
smoke test where kubelet/etcd failures are acceptable.
EOF
    if [[ "${DD_ALLOW_UNDERSIZED}" != "1" ]]; then
      exit 1
    fi
  fi

  if (( cpus < 4 || mem_mib < 15000 )); then
    cat >&2 <<EOF
WARNING: this node is smaller than the documented remote-dev target.
The cluster may come up, but thread pods request 1 CPU / 2 GiB each and
will not be reliable until the instance is resized to the larger x86 host.
EOF
  fi
}

install_base_packages() {
  echo "==> Installing base Amazon Linux packages"
  if [[ "${DD_SKIP_DNF_UPDATE}" != "1" ]]; then
    run_root dnf update -y
  fi

  run_root dnf install -y --allowerasing \
    bash-completion \
    bind-utils \
    conntrack-tools \
    curl \
    ethtool \
    findutils \
    git \
    gzip \
    iproute \
    iptables \
    iptables-nft \
    iputils \
    jq \
    openssl \
    procps-ng \
    socat \
    tar \
    unzip \
    util-linux \
    which \
    xz
}

configure_kernel() {
  echo "==> Configuring kernel modules and sysctl for Kubernetes"
  cat <<'EOF' | run_root tee /etc/modules-load.d/dd-k8s.conf >/dev/null
overlay
br_netfilter
ip_vs
ip_vs_rr
ip_vs_wrr
ip_vs_sh
nf_conntrack
EOF

  run_root modprobe overlay
  run_root modprobe br_netfilter
  run_root modprobe ip_vs || true
  run_root modprobe ip_vs_rr || true
  run_root modprobe ip_vs_wrr || true
  run_root modprobe ip_vs_sh || true
  run_root modprobe nf_conntrack || true

  cat <<'EOF' | run_root tee /etc/sysctl.d/99-dd-k8s.conf >/dev/null
net.bridge.bridge-nf-call-iptables = 1
net.bridge.bridge-nf-call-ip6tables = 1
net.ipv4.ip_forward = 1
fs.inotify.max_user_watches = 524288
fs.inotify.max_user_instances = 8192
fs.file-max = 2097152
net.netfilter.nf_conntrack_max = 1048576
EOF

  run_root sysctl --system
  run_root swapoff -a || true

  if [[ -f /etc/fstab ]]; then
    awk '
      /^[^#].*[[:space:]]swap[[:space:]]/ { print "# " $0; next }
      { print }
    ' /etc/fstab > /tmp/dd-k8s-fstab
    run_root cp /tmp/dd-k8s-fstab /etc/fstab
  fi
}

install_containerd() {
  if command -v containerd >/dev/null 2>&1; then
    echo "==> containerd already installed: $(containerd --version)"
  else
    echo "==> Installing containerd ${CONTAINERD_VERSION}"
    curl -fsSL "https://github.com/containerd/containerd/releases/download/v${CONTAINERD_VERSION}/containerd-${CONTAINERD_VERSION}-linux-${ARCH}.tar.gz" \
      | run_root tar -xzC /usr/local

    run_root mkdir -p /usr/local/lib/systemd/system
    curl -fsSL "https://raw.githubusercontent.com/containerd/containerd/v${CONTAINERD_VERSION}/containerd.service" \
      | run_root tee /usr/local/lib/systemd/system/containerd.service >/dev/null
  fi

  run_root mkdir -p /etc/containerd
  run_root containerd config default | run_root tee /etc/containerd/config.toml >/dev/null
  awk '{gsub(/SystemdCgroup = false/, "SystemdCgroup = true")}1' \
    /etc/containerd/config.toml > /tmp/dd-containerd-config.toml
  run_root cp /tmp/dd-containerd-config.toml /etc/containerd/config.toml

  echo "==> Installing runc ${RUNC_VERSION}"
  curl -fsSL "https://github.com/opencontainers/runc/releases/download/v${RUNC_VERSION}/runc.${ARCH}" \
    | run_root tee /usr/local/sbin/runc >/dev/null
  run_root chmod 755 /usr/local/sbin/runc

  echo "==> Installing CNI plugins ${CNI_VERSION}"
  run_root mkdir -p /opt/cni/bin
  curl -fsSL "https://github.com/containernetworking/plugins/releases/download/v${CNI_VERSION}/cni-plugins-linux-${ARCH}-v${CNI_VERSION}.tgz" \
    | run_root tar -xzC /opt/cni/bin

  echo "==> Installing crictl ${CRICTL_VERSION}"
  curl -fsSL "https://github.com/kubernetes-sigs/cri-tools/releases/download/v${CRICTL_VERSION}/crictl-v${CRICTL_VERSION}-linux-${ARCH}.tar.gz" \
    | run_root tar -xzC /usr/local/bin

  cat <<'EOF' | run_root tee /etc/crictl.yaml >/dev/null
runtime-endpoint: unix:///run/containerd/containerd.sock
image-endpoint: unix:///run/containerd/containerd.sock
timeout: 10
debug: false
EOF

  echo "==> Installing nerdctl ${NERDCTL_VERSION}"
  curl -fsSL "https://github.com/containerd/nerdctl/releases/download/v${NERDCTL_VERSION}/nerdctl-${NERDCTL_VERSION}-linux-${ARCH}.tar.gz" \
    | run_root tar -xzC /usr/local/bin

  run_root systemctl daemon-reload
  run_root systemctl enable --now containerd
}

install_kubernetes_packages() {
  echo "==> Installing Kubernetes ${K8S_VERSION} packages"
  cat <<EOF | run_root tee /etc/yum.repos.d/kubernetes.repo >/dev/null
[kubernetes]
name=Kubernetes
baseurl=https://pkgs.k8s.io/core:/stable:/v${K8S_VERSION}/rpm/
enabled=1
gpgcheck=1
gpgkey=https://pkgs.k8s.io/core:/stable:/v${K8S_VERSION}/rpm/repodata/repomd.xml.key
EOF

  run_root dnf install -y kubelet kubeadm kubectl
  run_root systemctl enable kubelet
}

install_cluster_tooling() {
  local cache_argocd_manifest="${1:-all}"
  local helm_tag helm_version cilium_tag hubble_tag argocd_tag

  echo "==> Installing Helm"
  helm_tag="$(curl -fsSL https://api.github.com/repos/helm/helm/releases/latest | jq -r .tag_name)"
  helm_version="${helm_tag#v}"
  curl -fsSL "https://get.helm.sh/helm-v${helm_version}-linux-${ARCH}.tar.gz" -o /tmp/dd-helm.tgz
  tar -xzf /tmp/dd-helm.tgz -C /tmp
  run_root install -m 0755 "/tmp/linux-${ARCH}/helm" /usr/local/bin/helm

  echo "==> Installing Cilium CLI"
  cilium_tag="$(curl -fsSL https://api.github.com/repos/cilium/cilium-cli/releases/latest | jq -r .tag_name)"
  curl -fsSL "https://github.com/cilium/cilium-cli/releases/download/${cilium_tag}/cilium-linux-${ARCH}.tar.gz" -o /tmp/dd-cilium.tgz
  tar -xzf /tmp/dd-cilium.tgz -C /tmp
  run_root install -m 0755 /tmp/cilium /usr/local/bin/cilium

  echo "==> Installing Hubble CLI"
  hubble_tag="$(curl -fsSL https://api.github.com/repos/cilium/hubble/releases/latest | jq -r .tag_name)"
  curl -fsSL "https://github.com/cilium/hubble/releases/download/${hubble_tag}/hubble-linux-${ARCH}.tar.gz" -o /tmp/dd-hubble.tgz
  tar -xzf /tmp/dd-hubble.tgz -C /tmp
  run_root install -m 0755 /tmp/hubble /usr/local/bin/hubble

  if [[ "${cache_argocd_manifest}" != "prereqs" ]]; then
    echo "==> Installing ArgoCD CLI and caching install manifest"
    argocd_tag="$(curl -fsSL https://api.github.com/repos/argoproj/argo-cd/releases/latest | jq -r .tag_name)"
    curl -fsSL "https://github.com/argoproj/argo-cd/releases/download/${argocd_tag}/argocd-linux-${ARCH}" -o /tmp/dd-argocd
    run_root install -m 0755 /tmp/dd-argocd /usr/local/bin/argocd
    run_root mkdir -p /opt/dd/manifests
    curl -fsSL "https://raw.githubusercontent.com/argoproj/argo-cd/${argocd_tag}/manifests/install.yaml" \
      | run_root tee /opt/dd/manifests/argocd-install.yaml >/dev/null
  fi
}

verify_prereqs() {
  echo ""
  echo "==> Installed tool versions"
  git --version
  containerd --version
  runc --version
  crictl --version
  kubectl version --client=true
  kubeadm version
  helm version --short
  cilium version --client || true
  hubble version || true
  if command -v argocd >/dev/null 2>&1; then
    argocd version --client --short 2>/dev/null || argocd version --client
  else
    echo "argocd: skipped in prereqs-only mode"
  fi
}

install_prereqs() {
  ensure_git_available
  sync_repo_to_origin_dev
  capacity_summary
  install_base_packages
  configure_kernel
  install_containerd
  install_kubernetes_packages
  install_cluster_tooling "${MODE}"
  verify_prereqs
}

ensure_cluster_inputs() {
  local secrets_file="${MANIFESTS_DIR}/02-secrets.yaml"

  if [[ ! -f "${MANIFESTS_DIR}/00-namespace.yaml" ]]; then
    echo "ERROR: manifests not found at ${MANIFESTS_DIR}" >&2
    exit 1
  fi

  if [[ ! -f "${secrets_file}" ]]; then
    cat >&2 <<EOF
ERROR: ${secrets_file} is missing.

Create it from ${MANIFESTS_DIR}/02-secrets.template.yaml, fill in real
REMOTE_DEV, provider, GitHub, Supabase, Redis, and storage secrets, then rerun.
EOF
    exit 1
  fi

  if grep -q "REPLACE_ME" "${secrets_file}"; then
    echo "ERROR: ${secrets_file} still contains REPLACE_ME placeholders." >&2
    exit 1
  fi
}

bootstrap_cluster() {
  ensure_git_available
  sync_repo_to_origin_dev
  check_capacity_for_cluster
  ensure_cluster_inputs
  run_root bash "${REPO_ROOT}/remote/ami/bootstrap-cluster.sh" \
    --pod-cidr "${POD_CIDR}" \
    --manifests-dir "${MANIFESTS_DIR}"
}

case "${MODE}" in
  prereqs)
    install_prereqs
    ;;
  cluster)
    bootstrap_cluster
    ;;
  all)
    install_prereqs
    bootstrap_cluster
    ;;
  *)
    echo "ERROR: unsupported mode: ${MODE}" >&2
    exit 2
    ;;
esac
