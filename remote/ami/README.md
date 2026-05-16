# DD K8s Node — AMI & Cluster Setup

## Overview

This directory contains everything needed to build an EC2 AMI and bootstrap
a self-managed K8s cluster for running `dd-dev-server` containers in the
container-per-thread model.

If you need to scaffold a plain Amazon Linux 2023 EC2 instance before this AMI
exists, use [`../ec2/bootstrap-amazon-linux-2023-k8s.sh`](../ec2/bootstrap-amazon-linux-2023-k8s.sh).
That script installs the host prerequisites and then calls this directory's
`bootstrap-cluster.sh`, so the final cluster shape stays the same.

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│  EC2 Instance (dd-k8s-node AMI)                         │
│                                                         │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐              │
│  │ Pod:     │  │ Pod:     │  │ Pod:     │   (1 per      │
│  │ thread-A │  │ thread-B │  │ thread-C │    thread)    │
│  │ :8080    │  │ :8080    │  │ :8080    │              │
│  └──────────┘  └──────────┘  └──────────┘              │
│       ▲                                                 │
│       │ K8s control plane routes by label               │
│       │ dd/threadId={uuid}                              │
│                                                         │
│  ┌────────────────────────────┐                         │
│  │ Cilium CNI (eBPF)          │                         │
│  │ ArgoCD UI (:30443)         │                         │
│  │ containerd + runc          │                         │
│  └────────────────────────────┘                         │
└─────────────────────────────────────────────────────────┘
         ▲
         │ K8S_API_SERVER + K8S_SA_TOKEN
         │
    Vercel (dispatch layer)
```

## Files

| File | Purpose |
|---|---|
| `k8s-dev-node.pkr.hcl` | Packer template — builds the Amazon Linux 2023 EC2 AMI by chaining the modular `scripts/*.sh` provisioners |
| `scripts/*.sh` | Provisioner phases for base packages, K8s, runtimes, networking, IaC, security, and validation |
| `bootstrap-cluster.sh` | Baked into the AMI as `dd-bootstrap-cluster`; run once on EC2 to init the K8s cluster |

## Quick Start

### 1. Build the AMI

```bash
cd remote/ami

# Install Packer plugin
packer init k8s-dev-node.pkr.hcl

# Build (takes ~30-45 min; Git/Python/Flutter source/download steps dominate)
packer build k8s-dev-node.pkr.hcl

# Or with custom region/instance:
packer build \
  -var 'aws_region=us-east-1' \
  -var 'instance_type=t3.2xlarge' \
  k8s-dev-node.pkr.hcl
```

### 2. Launch an EC2 Instance

Launch from the AMI with:
- **Instance type**: `x8i.2xlarge` recommended for the x86 path (8 vCPU, 128 GiB RAM)
- **Storage**: 800GB gp3
- **Security group**: Allow inbound 6443 (K8s API) + 30443 (ArgoCD)
- **IAM role**: ECR pull access for the dd-dev-server image

### 3. Bootstrap the Cluster

```bash
ssh ec2-user@<instance-ip>
sudo dd-bootstrap-cluster
```

The script outputs the Vercel env vars you need:
```
K8S_API_SERVER=https://<ip>:6443
K8S_NAMESPACE=dd-dev
K8S_SA_TOKEN=<token>
K8S_INSECURE_TLS=true
```

### 4. Configure Vercel

Add those env vars to your Vercel project. The dispatch layer will now
use K8s as the primary orchestrator.

### 5. Verify

```bash
# On the EC2 instance:
kubectl get nodes          # Should show 1 Ready node
cilium status              # Should show OK
kubectl get pods -A        # ArgoCD + Cilium pods
k9s                        # Interactive TUI dashboard

# ArgoCD UI:
open https://<instance-ip>:30443
# Username: admin
# Password: kubectl -n argocd get secret argocd-initial-admin-secret \
#            -o jsonpath='{.data.password}' | base64 -d
```

## Pre-installed Tools

### Languages & Runtimes
Node.js (via NVM), Python 2 & 3, Go, Rust, Gleam, Elixir, Flutter/Dart,
Java (Corretto), TypeScript, pnpm

### K8s & Containers
kubectl, kubeadm, kubelet, helm, containerd, crictl, ctr, nerdctl,
etcdctl, minikube, k9s, stern, ArgoCD, Cilium CLI, Hubble

### AWS & IaC
AWS CLI v2, AWS CDK, Terraform, Ansible

### Networking & Debug
tcpdump, traceroute, nmap, netcat, socat, mtr, iptables, iproute,
conntrack-tools

### eBPF & Security
bpftool, bpftrace, bcc-tools, Trivy, fio

### Utilities
git, mercurial, jq, yq, tmux, screen, rsync, gcc, cmake
