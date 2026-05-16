# ─────────────────────────────────────────────────────────────
# DD K8s Dev-Node AMI — Packer HCL2 Template
#
# Builds an Amazon Linux 2023 AMI with:
#   • Full Kubernetes stack (containerd + kubeadm + minikube)
#   • Polyglot dev toolchain (Node, Python 2/3, Go, Rust, Java,
#     Elixir, Gleam, Dart, Flutter)
#   • Cloud IaC (Terraform, Ansible, AWS CDK)
#   • eBPF / CNI observability (Cilium, Hubble, bpftrace, bcc)
#   • Networking diagnostics (tcpdump, nmap, socat, mtr, …)
#   • Security scanning (Trivy) + benchmarking (fio)
#
# Usage:
#   packer init  k8s-dev-node.pkr.hcl
#   packer build k8s-dev-node.pkr.hcl
# ─────────────────────────────────────────────────────────────

packer {
  required_plugins {
    amazon = {
      version = ">= 1.3.0"
      source  = "github.com/hashicorp/amazon"
    }
  }
}

# ─────────────────────────────────────────────────────────────
# Variables
# ─────────────────────────────────────────────────────────────

variable "aws_region" {
  type    = string
  default = "us-east-1"
}

variable "instance_type" {
  type        = string
  default     = "m5.xlarge"
  description = "Builder instance — 4 vCPU / 16 GB recommended for compilation steps"
}

variable "ami_name_prefix" {
  type    = string
  default = "dd-k8s-dev-node"
}

variable "vpc_id" {
  type        = string
  default     = ""
  description = "VPC to build in (empty = default VPC)"
}

variable "subnet_id" {
  type        = string
  default     = ""
  description = "Subnet to build in (empty = auto-select)"
}

variable "node_version" {
  type        = string
  default     = "22"
  description = "Node.js LTS major version installed via nvm"
}

variable "go_version" {
  type    = string
  default = "1.23.0"
}

variable "k8s_version" {
  type        = string
  default     = "1.31"
  description = "Kubernetes minor version for kubeadm/kubectl/kubelet repos"
}

variable "rust_profile" {
  type        = string
  default     = "default"
  description = "rustup profile: minimal | default | complete"
}

variable "java_version" {
  type        = string
  default     = "21"
  description = "Amazon Corretto JDK major version"
}

variable "gleam_version" {
  type    = string
  default = "1.6.1"
}

variable "elixir_version" {
  type        = string
  default     = "1.17.3"
  description = "Elixir version installed from precompiled release assets; use latest to resolve at build time"
}

variable "minikube_version" {
  type    = string
  default = "latest"
}

variable "tags" {
  type = map(string)
  default = {
    Project     = "dancing-dragons"
    Component   = "dev-server"
    ManagedBy   = "packer"
  }
}

# ─────────────────────────────────────────────────────────────
# Locals
# ─────────────────────────────────────────────────────────────

locals {
  timestamp = formatdate("YYYYMMDD-hhmm", timestamp())
  ami_name  = "${var.ami_name_prefix}-${local.timestamp}"
}

# ─────────────────────────────────────────────────────────────
# Source: Amazon EBS-backed AMI from Amazon Linux 2023
# ─────────────────────────────────────────────────────────────

source "amazon-ebs" "k8s_dev_node" {
  region        = var.aws_region
  instance_type = var.instance_type

  # Dynamically find the latest AL2023 x86_64 AMI
  source_ami_filter {
    filters = {
      name                = "al2023-ami-2023.*-x86_64"
      root-device-type    = "ebs"
      virtualization-type = "hvm"
      architecture        = "x86_64"
    }
    owners      = ["amazon"]
    most_recent = true
  }

  # Networking (empty strings → Packer auto-selects default VPC/subnet)
  vpc_id    = var.vpc_id != "" ? var.vpc_id : null
  subnet_id = var.subnet_id != "" ? var.subnet_id : null

  associate_public_ip_address = true
  ssh_username                = "ec2-user"
  ssh_timeout                 = "10m"

  # AMI configuration
  ami_name        = local.ami_name
  ami_description = "DD K8s dev node — polyglot toolchain + containerd + kubeadm + minikube (${local.timestamp})"

  ami_block_device_mappings {
    device_name           = "/dev/xvda"
    volume_size           = 800
    volume_type           = "gp3"
    throughput            = 250
    iops                  = 4000
    delete_on_termination = true
  }

  tags = merge(var.tags, {
    Name      = local.ami_name
    BuildDate = local.timestamp
  })

  run_tags = merge(var.tags, {
    Name = "packer-builder-${local.ami_name}"
  })
}

# ─────────────────────────────────────────────────────────────
# Build
# ─────────────────────────────────────────────────────────────

build {
  name    = "dd-k8s-dev-node"
  sources = ["source.amazon-ebs.k8s_dev_node"]

  # ── 01: Base system packages, kernel modules, sysctl ──
  provisioner "shell" {
    script = "${path.root}/scripts/01-base-system.sh"
    environment_vars = [
      "DEBIAN_FRONTEND=noninteractive",
    ]
    execute_command = "sudo -S env {{ .Vars }} bash '{{ .Path }}'"
  }

  # ── 02: Container runtime — containerd, crictl, nerdctl, ctr ──
  provisioner "shell" {
    script = "${path.root}/scripts/02-container-runtime.sh"
    environment_vars = [
      "K8S_VERSION=${var.k8s_version}",
    ]
    execute_command = "sudo -S env {{ .Vars }} bash '{{ .Path }}'"
  }

  # ── 03: Kubernetes — kubectl, kubeadm, kubelet, minikube, helm, etcdctl, k9s, stern, ArgoCD CLI ──
  provisioner "shell" {
    script = "${path.root}/scripts/03-kubernetes.sh"
    environment_vars = [
      "K8S_VERSION=${var.k8s_version}",
      "MINIKUBE_VERSION=${var.minikube_version}",
    ]
    execute_command = "sudo -S env {{ .Vars }} bash '{{ .Path }}'"
  }

  # ── 04: Networking & eBPF — Cilium, Hubble, bpftool, bpftrace, bcc, tcpdump, nmap, etc. ──
  provisioner "shell" {
    script = "${path.root}/scripts/04-networking.sh"
    execute_command = "sudo -S env {{ .Vars }} bash '{{ .Path }}'"
  }

  # ── 05: Languages — Node/nvm, Python 2/3, Go, Rust, Java, Elixir, Gleam, Dart, Flutter, GCC ──
  provisioner "shell" {
    script = "${path.root}/scripts/05-languages.sh"
    environment_vars = [
      "NODE_VERSION=${var.node_version}",
      "GO_VERSION=${var.go_version}",
      "RUST_PROFILE=${var.rust_profile}",
      "JAVA_VERSION=${var.java_version}",
      "GLEAM_VERSION=${var.gleam_version}",
      "ELIXIR_VERSION=${var.elixir_version}",
    ]
    # Run as ec2-user so nvm, rustup, pip --user, and Flutter caches land in the right home dir.
    execute_command = "sudo -u ec2-user env {{ .Vars }} bash '{{ .Path }}'"
  }

  # ── 06: Cloud & IaC — AWS CLI v2, CDK, Terraform, Ansible ──
  provisioner "shell" {
    script = "${path.root}/scripts/06-cloud-iac.sh"
    execute_command = "sudo -S env {{ .Vars }} bash '{{ .Path }}'"
  }

  # ── 07: Security & Benchmarking — Trivy, fio ──
  provisioner "shell" {
    script = "${path.root}/scripts/07-security-bench.sh"
    execute_command = "sudo -S env {{ .Vars }} bash '{{ .Path }}'"
  }

  # ── Cluster bootstrap helper — available as dd-bootstrap-cluster on the EC2 node ──
  provisioner "file" {
    source      = "${path.root}/bootstrap-cluster.sh"
    destination = "/tmp/bootstrap-cluster.sh"
  }

  provisioner "shell" {
    inline = [
      "sudo install -D -m 0755 /tmp/bootstrap-cluster.sh /opt/dd/bin/bootstrap-cluster.sh",
      "sudo ln -sf /opt/dd/bin/bootstrap-cluster.sh /usr/local/bin/dd-bootstrap-cluster",
    ]
  }

  # ── 08: Cleanup — shrink image size ──
  provisioner "shell" {
    script = "${path.root}/scripts/08-cleanup.sh"
    execute_command = "sudo -S env {{ .Vars }} bash '{{ .Path }}'"
  }

  # ── Validation ──
  post-processor "manifest" {
    output     = "build-manifest.json"
    strip_path = true
  }
}
