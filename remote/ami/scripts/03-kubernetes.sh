#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# 03-kubernetes.sh - kubelet, kubeadm, kubectl, Helm, minikube, etcdctl,
#                    k9s, stern, and ArgoCD CLI/manifests
# Runs as: root
# Env:
#   K8S_VERSION       - Kubernetes minor version, for example "1.31"
#   MINIKUBE_VERSION  - "latest" or a version without leading v
# ---------------------------------------------------------------------------
set -euo pipefail

echo "=========================================================="
echo "  03 - Kubernetes Tooling"
echo "=========================================================="

ARCH="amd64"
K8S_VERSION="${K8S_VERSION:-1.31}"
MINIKUBE_VERSION="${MINIKUBE_VERSION:-latest}"

echo "Installing Kubernetes ${K8S_VERSION} packages..."
cat > /etc/yum.repos.d/kubernetes.repo <<EOF
[kubernetes]
name=Kubernetes
baseurl=https://pkgs.k8s.io/core:/stable:/v${K8S_VERSION}/rpm/
enabled=1
gpgcheck=1
gpgkey=https://pkgs.k8s.io/core:/stable:/v${K8S_VERSION}/rpm/repodata/repomd.xml.key
EOF

dnf install -y kubelet kubeadm kubectl
systemctl enable kubelet

echo "Installing Helm..."
HELM_TAG=$(curl -fsSL https://api.github.com/repos/helm/helm/releases/latest | jq -r .tag_name)
HELM_VERSION="${HELM_TAG#v}"
curl -fsSL "https://get.helm.sh/helm-v${HELM_VERSION}-linux-${ARCH}.tar.gz" -o /tmp/helm.tgz
tar -xzf /tmp/helm.tgz -C /tmp
install -m 0755 "/tmp/linux-${ARCH}/helm" /usr/local/bin/helm

echo "Installing etcdctl..."
ETCD_TAG=$(curl -fsSL https://api.github.com/repos/etcd-io/etcd/releases/latest | jq -r .tag_name)
curl -fsSL "https://github.com/etcd-io/etcd/releases/download/${ETCD_TAG}/etcd-${ETCD_TAG}-linux-${ARCH}.tar.gz" -o /tmp/etcd.tgz
tar -xzf /tmp/etcd.tgz -C /tmp
install -m 0755 "/tmp/etcd-${ETCD_TAG}-linux-${ARCH}/etcdctl" /usr/local/bin/etcdctl

echo "Installing minikube..."
if [[ "${MINIKUBE_VERSION}" == "latest" ]]; then
  curl -fsSL "https://storage.googleapis.com/minikube/releases/latest/minikube-linux-${ARCH}" -o /tmp/minikube
else
  curl -fsSL "https://github.com/kubernetes/minikube/releases/download/v${MINIKUBE_VERSION}/minikube-linux-${ARCH}" -o /tmp/minikube
fi
install -m 0755 /tmp/minikube /usr/local/bin/minikube

echo "Installing k9s..."
K9S_TAG=$(curl -fsSL https://api.github.com/repos/derailed/k9s/releases/latest | jq -r .tag_name)
curl -fsSL "https://github.com/derailed/k9s/releases/download/${K9S_TAG}/k9s_Linux_${ARCH}.tar.gz" -o /tmp/k9s.tgz
tar -xzf /tmp/k9s.tgz -C /tmp
install -m 0755 /tmp/k9s /usr/local/bin/k9s

echo "Installing stern..."
STERN_TAG=$(curl -fsSL https://api.github.com/repos/stern/stern/releases/latest | jq -r .tag_name)
STERN_VERSION="${STERN_TAG#v}"
curl -fsSL "https://github.com/stern/stern/releases/download/${STERN_TAG}/stern_${STERN_VERSION}_linux_${ARCH}.tar.gz" -o /tmp/stern.tgz
tar -xzf /tmp/stern.tgz -C /tmp
install -m 0755 /tmp/stern /usr/local/bin/stern

echo "Installing ArgoCD CLI and caching install manifest..."
ARGOCD_TAG=$(curl -fsSL https://api.github.com/repos/argoproj/argo-cd/releases/latest | jq -r .tag_name)
curl -fsSL "https://github.com/argoproj/argo-cd/releases/download/${ARGOCD_TAG}/argocd-linux-${ARCH}" -o /tmp/argocd
install -m 0755 /tmp/argocd /usr/local/bin/argocd
mkdir -p /opt/dd/manifests
curl -fsSL "https://raw.githubusercontent.com/argoproj/argo-cd/${ARGOCD_TAG}/manifests/install.yaml" \
  -o /opt/dd/manifests/argocd-install.yaml

echo ""
echo "--- Kubernetes Tool Versions ---"
kubectl version --client=true
kubeadm version
helm version --short
etcdctl version | head -1
minikube version --short
k9s version --short 2>/dev/null || k9s version
stern --version
argocd version --client --short 2>/dev/null || argocd version --client

echo "03 - Kubernetes tooling installation complete"
