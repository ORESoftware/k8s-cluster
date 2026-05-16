#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# 08-cleanup.sh - final PATH/profile setup, service enablement, light cleanup,
#                 and build-time validation output
# Runs as: root
# ---------------------------------------------------------------------------
set -euo pipefail

echo "=========================================================="
echo "  08 - Final Profile, Cleanup, and Validation"
echo "=========================================================="

cat > /etc/profile.d/dd-k8s-node.sh <<'PROFILE'
# DD K8s EC2 node - common developer and cluster PATH setup.
export PATH="$PATH:/usr/local/go/bin:$HOME/go/bin:$HOME/.local/bin"
export PATH="$PATH:/opt/flutter/bin:/opt/flutter/bin/cache/dart-sdk/bin"
export NVM_DIR="$HOME/.nvm"
[ -s "$NVM_DIR/nvm.sh" ] && . "$NVM_DIR/nvm.sh"
[ -s "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
if command -v npm >/dev/null 2>&1; then
  export NODE_PATH="$(npm root -g 2>/dev/null):${NODE_PATH:-}"
fi
alias k=kubectl
if [ -n "${BASH_VERSION:-}" ]; then
  source <(kubectl completion bash 2>/dev/null) || true
  source <(helm completion bash 2>/dev/null) || true
  complete -o default -F __start_kubectl k 2>/dev/null || true
fi
PROFILE

chmod 0644 /etc/profile.d/dd-k8s-node.sh

systemctl enable containerd
systemctl enable kubelet

dnf clean all
find /tmp -mindepth 1 -maxdepth 1 -type f -delete || true

export PATH="$PATH:/usr/local/go/bin:/home/ec2-user/go/bin:/home/ec2-user/.local/bin"
export PATH="$PATH:/opt/flutter/bin:/opt/flutter/bin/cache/dart-sdk/bin"
export NVM_DIR="/home/ec2-user/.nvm"
[ -s "$NVM_DIR/nvm.sh" ] && source "$NVM_DIR/nvm.sh"
[ -s "/home/ec2-user/.cargo/env" ] && source /home/ec2-user/.cargo/env

echo ""
echo "=========================================================="
echo "  DD K8s EC2 AMI - Installed Tool Versions"
echo "=========================================================="
echo -n "git:        " && git --version
echo -n "hg:         " && hg --version | head -1
echo -n "python2:    " && python2 --version 2>&1
echo -n "python3:    " && python3 --version
echo -n "node:       " && node --version
echo -n "nvm:        " && nvm --version
echo -n "pnpm:       " && pnpm --version
echo -n "tsc:        " && tsc --version
echo -n "go:         " && go version
echo -n "rustc:      " && rustc --version
echo -n "cargo:      " && cargo --version
echo -n "gleam:      " && gleam --version
echo -n "elixir:     " && elixir --version | head -1
echo -n "dart:       " && dart --version 2>&1
echo -n "flutter:    " && flutter --version | head -1
echo -n "java:       " && java -version 2>&1 | head -1
echo -n "gcc:        " && gcc --version | head -1
echo -n "aws:        " && aws --version
echo -n "cdk:        " && cdk --version
echo -n "terraform:  " && terraform version | head -1
echo -n "ansible:    " && ansible --version | head -1
echo -n "kubectl:    " && kubectl version --client=true
echo -n "kubeadm:    " && kubeadm version
echo -n "helm:       " && helm version --short
echo -n "containerd: " && containerd --version
echo -n "ctr:        " && ctr --version
echo -n "crictl:     " && crictl --version
echo -n "nerdctl:    " && nerdctl --version
echo -n "etcdctl:    " && etcdctl version | head -1
echo -n "minikube:   " && minikube version --short
echo -n "k9s:        " && k9s version --short 2>/dev/null || k9s version
echo -n "stern:      " && stern --version
echo -n "argocd:     " && argocd version --client --short 2>/dev/null || argocd version --client
echo -n "cilium:     " && cilium version --client 2>/dev/null || true
echo -n "hubble:     " && hubble version 2>/dev/null || true
echo -n "jq:         " && jq --version
echo -n "yq:         " && yq --version
echo -n "tmux:       " && tmux -V
echo -n "screen:     " && screen --version | head -1
echo -n "rsync:      " && rsync --version | head -1
echo -n "tcpdump:    " && tcpdump --version | head -1
echo -n "traceroute: " && traceroute --version 2>&1 | head -1
echo -n "netcat:     " && nc -h 2>&1 | head -1
echo -n "socat:      " && socat -V | head -1
echo -n "nmap:       " && nmap --version | head -1
echo -n "mtr:        " && mtr --version
echo -n "iptables:   " && iptables --version
echo -n "iproute:    " && ip -V
echo -n "conntrack:  " && conntrack -V
echo -n "bpftool:    " && bpftool version || true
echo -n "bpftrace:   " && bpftrace --version || true
echo -n "fio:        " && fio --version
echo -n "trivy:      " && trivy --version | head -1
echo "=========================================================="

echo "08 - AMI finalization complete"
